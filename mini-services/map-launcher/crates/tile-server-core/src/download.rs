//! Region downloads, craft-launcher style: a catalog of Tanzania map parts
//! (plus the whole country) fetched over HTTPS with progress, reconnect and
//! resume, then cropped into per-region stores with the same extractor the
//! CLI uses. The UI polls [`Manager::all`] for the live job phases.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Geofabrik's whole-Tanzania extract (the shared source PBF for every part).
pub const TANZANIA_PBF_URL: &str = "https://download.geofabrik.de/africa/tanzania-latest.osm.pbf";

/// One downloadable part of Tanzania (or the whole country).
#[derive(Clone, Debug)]
pub struct Region {
    /// Stable id (also the store folder name under `regions/`).
    pub id: &'static str,
    /// Card title.
    pub name: &'static str,
    /// Card blurb.
    pub desc: &'static str,
    /// Crop box `[w, s, e, n]`.
    pub bbox: [f64; 4],
    /// Source PBF URL. Parts share the whole-country PBF (`shared: true`).
    pub url: &'static str,
    /// `true` when the crop comes out of the shared Tanzania PBF.
    pub shared: bool,
    /// Human size hint for the card.
    pub size_hint: &'static str,
}

/// The catalog: the whole country first, then its parts.
pub fn regions() -> Vec<Region> {
    vec![
        Region {
            id: "whole-tanzania",
            name: "Tanzania (whole country)",
            desc: "Every highway, building and POI of Tanzania in one store. Largest download; the source PBF other regions crop from.",
            bbox: [29.2, -11.8, 40.7, -0.9],
            url: TANZANIA_PBF_URL,
            shared: false,
            size_hint: "~730 MB PBF",
        },
        Region {
            id: "central-tanzania",
            name: "Central Tanzania (Dodoma)",
            desc: "The capital region around Dodoma — the default region this server ships with.",
            bbox: carto_core::DODOMA_BBOX,
            url: TANZANIA_PBF_URL,
            shared: true,
            size_hint: "~40 MB store",
        },
        Region {
            id: "dar-es-salaam",
            name: "Dar es Salaam & Coast",
            desc: "The commercial capital, Bagamoyo and the Indian Ocean coastline.",
            bbox: [38.6, -7.7, 39.7, -6.3],
            url: TANZANIA_PBF_URL,
            shared: true,
            size_hint: "~60 MB store",
        },
        Region {
            id: "northern-tanzania",
            name: "Northern Tanzania",
            desc: "Arusha, Moshi, Kilimanjaro and the gateways to the northern safari circuit.",
            bbox: [35.3, -4.5, 38.2, -2.2],
            url: TANZANIA_PBF_URL,
            shared: true,
            size_hint: "~70 MB store",
        },
        Region {
            id: "lake-zone",
            name: "Lake Zone (Victoria)",
            desc: "Mwanza, the south shore of Lake Victoria and the gold belt around Geita.",
            bbox: [31.4, -3.6, 34.8, -1.0],
            url: TANZANIA_PBF_URL,
            shared: true,
            size_hint: "~55 MB store",
        },
        Region {
            id: "southern-highlands",
            name: "Southern Highlands",
            desc: "Mbeya, Njombe and the tea and coffee highlands towards Malawi and Zambia.",
            bbox: [32.3, -10.0, 35.0, -7.0],
            url: TANZANIA_PBF_URL,
            shared: true,
            size_hint: "~45 MB store",
        },
    ]
}

/// A live download/extract request for [`Manager::start`].
#[derive(Clone, Debug)]
pub struct RegionReq {
    pub id: String,
    pub name: String,
    pub bbox: [f64; 4],
    pub url: String,
}

/// What a job is doing right now (craft-launcher's `JobPhase`, adapted).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "phase", content = "data", rename_all = "snake_case")]
pub enum Phase {
    /// No job for this region.
    Idle,
    /// Queued behind the running job.
    Queued,
    Downloading {
        done: u64,
        total: u64,
    },
    /// The connection dropped; retrying shortly from where it stopped.
    Reconnecting {
        done: u64,
        total: u64,
    },
    /// Parsing the PBF and cropping to the region bbox.
    Extracting {
        message: String,
    },
    /// Store built and ready to serve.
    Installed {
        roads: usize,
        buildings: usize,
        pois: usize,
    },
    Failed {
        message: String,
    },
}

impl Phase {
    pub fn is_active(&self) -> bool {
        matches!(self, Phase::Queued | Phase::Downloading { .. } | Phase::Reconnecting { .. } | Phase::Extracting { .. })
    }
}

/// A counter fed by the PBF reader loop so jobs report live progress.
struct Progress {
    id: String,
    jobs: Arc<Mutex<HashMap<String, Phase>>>,
    cancel: Arc<AtomicBool>,
    last_emit: Instant,
}

impl Progress {
    fn tick(&mut self, done: u64, total: u64) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.last_emit) >= Duration::from_millis(250) || done == total {
            self.last_emit = now;
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(self.id.clone(), Phase::Downloading { done, total });
        }
    }
}

/// Runs region downloads and extractions in background threads; the UI polls.
pub struct Manager {
    jobs: Arc<Mutex<HashMap<String, Phase>>>,
    cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

impl Manager {
    pub fn new() -> Self {
        Self { jobs: Arc::new(Mutex::new(HashMap::new())), cancels: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// A snapshot of every known job phase.
    pub fn all(&self) -> HashMap<String, Phase> {
        self.jobs.lock().unwrap().clone()
    }

    pub fn phase(&self, id: &str) -> Phase {
        self.jobs.lock().unwrap().get(id).cloned().unwrap_or(Phase::Idle)
    }

    pub fn cancel(&self, id: &str) {
        if let Some(flag) = self.cancels.lock().unwrap().get(id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Launches the download + extract pipeline for `req` in a background
    /// thread (no-op when that region already has a running job).
    pub fn start(&self, req: RegionReq, downloads_dir: PathBuf, regions_dir: PathBuf) {
        {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.get(&req.id).is_some_and(Phase::is_active) {
                return;
            }
            jobs.insert(req.id.clone(), Phase::Queued);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancels.lock().unwrap().insert(req.id.clone(), cancel.clone());
        let jobs = self.jobs.clone();
        let id = req.id.clone();
        let cancels = self.cancels.clone();
        std::thread::spawn(move || {
            let result = run_job(&req, &downloads_dir, &regions_dir, &jobs, &cancel);
            {
                let mut j = jobs.lock().unwrap();
                match result {
                    Ok(counts) => {
                        j.insert(id.clone(), Phase::Installed { roads: counts.0, buildings: counts.1, pois: counts.2 });
                    }
                    Err(_e) if cancel.load(Ordering::Relaxed) => {
                        j.insert(id.clone(), Phase::Idle);
                    }
                    Err(e) => {
                        j.insert(id.clone(), Phase::Failed { message: e });
                    }
                }
            }
            cancels.lock().unwrap().remove(&id);
        });
    }
}

/// The worker: fetch the PBF (with resume), then crop the region store.
fn run_job(
    req: &RegionReq,
    downloads_dir: &Path,
    regions_dir: &Path,
    jobs: &Arc<Mutex<HashMap<String, Phase>>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(usize, usize, usize), String> {
    let _ = std::fs::create_dir_all(downloads_dir);
    let pbf_name = pbf_name_for(&req.url);
    let pbf_path = downloads_dir.join(&pbf_name);
    if !pbf_path.is_file() {
        download_with_resume(&req.url, &pbf_path, &req.id, jobs, cancel)?;
    } else {
        let mut j = jobs.lock().unwrap();
        j.insert(req.id.clone(), Phase::Downloading { done: pbf_path.metadata().map(|m| m.len()).unwrap_or(0), total: pbf_path.metadata().map(|m| m.len()).unwrap_or(0) });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    jobs.lock().unwrap().insert(req.id.clone(), Phase::Extracting { message: "Parsing OSM data and cropping to the region…".into() });
    let out = regions_dir.join(&req.id);
    let store = carto_core::extract(&pbf_path, &out, req.bbox).map_err(|e| format!("extract failed: {e}"))?;
    Ok((store.roads.len(), store.buildings.len(), store.pois.len()))
}

/// The file name a URL points at (used for the shared PBF cache).
pub fn pbf_name_for(url: &str) -> String {
    url.rsplit('/').next().unwrap_or("map.osm.pbf").split('?').next().unwrap_or("map.osm.pbf").to_string()
}

/// HTTPS GET with progress, reconnect and Range resume, like the launcher.
pub fn download_with_resume(
    url: &str,
    dest: &Path,
    id: &str,
    jobs: &Arc<Mutex<HashMap<String, Phase>>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let part = dest.with_extension("part");
    for attempt in 0..6u32 {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let partial_len = part.metadata().map(|m| m.len()).unwrap_or(0);
        let mut req = ureq::get(url);
        if partial_len > 0 {
            req = req.header("Range", &format!("bytes={partial_len}-"));
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(e) => {
                if attempt + 1 == 6 {
                    return Err(format!("download failed: {e}"));
                }
                jobs.lock().unwrap().insert(id.to_string(), Phase::Reconnecting { done: partial_len, total: 0 });
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        let status = resp.status().as_u16();
        // 206 = resumed; 200 = server ignored the range, restart the file.
        let resume = status == 206;
        let total = total_length(&resp, partial_len, resume);
        let mut reader = resp.into_body().into_reader();
        let mut file = if resume {
            let mut f = std::fs::OpenOptions::new().append(true).open(&part).map_err(|e| e.to_string())?;
            f.seek(SeekFrom::End(0)).ok();
            f
        } else {
            std::fs::File::create(&part).map_err(|e| e.to_string())?
        };
        let mut progress = Progress { id: id.to_string(), jobs: jobs.clone(), cancel: cancel.clone(), last_emit: Instant::now() };
        let mut done = if resume { partial_len } else { 0 };
        progress.tick(done, total);
        let mut buf = [0u8; 64 * 1024];
        #[allow(unused_assignments)]
        let mut last_err: Option<String> = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            match reader.read(&mut buf) {
                Ok(0) => {
                    last_err = None;
                    break;
                }
                Ok(n) => {
                    if file.write_all(&buf[..n]).is_err() {
                        last_err = Some("disk write failed".into());
                        break;
                    }
                    done += n as u64;
                    progress.tick(done, total);
                }
                Err(e) => {
                    last_err = Some(format!("connection lost: {e}"));
                    break;
                }
            }
        }
        match last_err {
            None => {
                std::fs::rename(&part, dest).map_err(|e| format!("rename failed: {e}"))?;
                return Ok(());
            }
            Some(msg) => {
                if attempt + 1 == 6 {
                    return Err(msg);
                }
                jobs.lock().unwrap().insert(id.to_string(), Phase::Reconnecting { done, total });
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    Err("download failed: too many reconnect attempts".into())
}

/// The expected total size from Content-Length / Content-Range.
fn total_length(resp: &ureq::http::Response<ureq::Body>, partial: u64, resume: bool) -> u64 {
    let headers = resp.headers();
    if resume {
        if let Some(cr) = headers.get("content-range").and_then(|v| v.to_str().ok()) {
            if let Some(total) = cr.rsplit('/').next().and_then(|t| t.parse::<u64>().ok()) {
                return total;
            }
        }
        return partial;
    }
    headers.get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
}
