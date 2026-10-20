//! Sprite engine: rasterize the embedded openstreetmap-carto SVG symbol set
//! (POI icons) and polygon pattern fills into cached tiny-skia pixmaps.
//!
//! All `assets/symbols/**` SVGs and the `assets/patterns/**` fills ship inside
//! the binary; rendering a tile only ever touches this cache, never the
//! filesystem.

use carto_style::{EMBEDDED_PATTERNS, EMBEDDED_SYMBOLS};
use std::collections::HashMap;
use std::sync::Mutex;
use tiny_skia::Pixmap;

/// Cached, pre-rasterized sprites keyed by path.
pub struct Sprites {
    icons: Mutex<HashMap<String, Option<Pixmap>>>,
    patterns: Mutex<HashMap<String, Option<Pixmap>>>,
}

fn load_svg(dir: &include_dir::Dir<'static>, path: &str) -> Option<Pixmap> {
    let file = dir.get_file(path)?;
    let bytes = file.contents();
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).ok()?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    let (w, h) = (size.width().ceil() as u32, size.height().ceil() as u32);
    if w == 0 || h == 0 || w > 512 || h > 512 {
        return None;
    }
    let mut pixmap = Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    Some(pixmap)
}

impl Sprites {
    pub fn new() -> Self {
        Self { icons: Mutex::new(HashMap::new()), patterns: Mutex::new(HashMap::new()) }
    }

    /// Rasterize `symbols/<path>` (e.g. `amenity/cafe.svg`) at natural size;
    /// callers scale it when blitting.
    pub fn icon(&self, rel_path: &str) -> Option<Pixmap> {
        if let Some(hit) = self.icons.lock().unwrap().get(rel_path) {
            return hit.clone();
        }
        let rendered = load_svg(&EMBEDDED_SYMBOLS, rel_path);
        self.icons.lock().unwrap().insert(rel_path.to_string(), rendered.clone());
        rendered
    }

    /// A polygon pattern tile (patterns/ dir first, then root symbol
    /// patterns like `leaftype_mixed.svg`) at its natural size.
    pub fn pattern(&self, rel_path: &str) -> Option<Pixmap> {
        if let Some(hit) = self.patterns.lock().unwrap().get(rel_path) {
            return hit.clone();
        }
        let rendered = EMBEDDED_PATTERNS
            .get_file(rel_path)
            .and_then(|_| load_svg(&EMBEDDED_PATTERNS, rel_path))
            .or_else(|| load_svg(&EMBEDDED_SYMBOLS, rel_path));
        self.patterns.lock().unwrap().insert(rel_path.to_string(), rendered.clone());
        rendered
    }
}

/// The POI icon lookup: category (`amenity_cafe`) -> symbols path. Only paths
/// that exist in the embedded set are listed; unknown categories degrade to a
/// plain carto dot.
pub fn poi_icon(category: &str) -> Option<&'static str> {
    Some(match category {
        /* ---- amenity ---- */
        "amenity_hospital" => "amenity/hospital.svg",
        "amenity_clinic" => "amenity/doctors.svg",
        "amenity_pharmacy" => "amenity/pharmacy.svg",
        "amenity_doctors" => "amenity/doctors.svg",
        "amenity_dentist" => "amenity/dentist.svg",
        "amenity_place_of_worship" => "amenity/place_of_worship.svg",
        "amenity_college" => "amenity/college.svg",
        "amenity_university" => "amenity/college.svg",
        "amenity_library" => "amenity/library.svg",
        "amenity_cinema" => "amenity/cinema.svg",
        "amenity_nightclub" => "amenity/nightclub.svg",
        "amenity_courthouse" => "amenity/courthouse.svg",
        "amenity_townhall" => "amenity/townhall.svg",
        "amenity_police" => "amenity/police.svg",
        "amenity_fire_station" => "amenity/firestation.svg",
        "amenity_prison" => "amenity/prison.svg",
        "amenity_post_office" => "amenity/post_office.svg",
        "amenity_post_box" => "amenity/post_box.svg",
        "amenity_bank" => "amenity/bank.svg",
        "amenity_atm" => "amenity/atm.svg",
        "amenity_bureau_de_change" => "amenity/bureau_de_change.svg",
        "amenity_cafe" => "amenity/cafe.svg",
        "amenity_bar" => "amenity/bar.svg",
        "amenity_pub" => "amenity/pub.svg",
        "amenity_biergarten" => "amenity/biergarten.svg",
        "amenity_ice_cream" => "amenity/ice_cream.svg",
        "amenity_fast_food" => "amenity/fast_food.svg",
        "amenity_internet_cafe" => "amenity/internet_cafe.svg",
        "amenity_fuel" => "amenity/fuel.svg",
        "amenity_charging_station" => "amenity/charging_station.svg",
        "amenity_bus_station" => "amenity/bus_station.svg",
        "amenity_ferry_terminal" => "amenity/ferry.svg",
        "amenity_aerodrome" => "amenity/aerodrome.svg",
        "amenity_helipad" => "amenity/helipad.svg",
        "amenity_marketplace" => "shop/marketplace.svg",
        "amenity_community_centre" => "amenity/community_centre.svg",
        "amenity_social_facility" => "amenity/community_centre.svg",
        "amenity_drinking_water" => "amenity/drinking_water.svg",
        "amenity_bench" => "amenity/bench.svg",
        "amenity_bicycle_parking" => "amenity/bicycle_parking.svg",
        "amenity_bicycle_repair_station" => "amenity/bicycle_repair_station.svg",
        "amenity_motorcycle_parking" => "amenity/motorcycle_parking.svg",
        "amenity_car_wash" => "amenity/car_wash.svg",
        "amenity_recycling" => "amenity/recycling.svg",
        "amenity_emergency_phone" => "amenity/emergency_phone.svg",
        "amenity_casino" => "amenity/casino.svg",
        "amenity_bbq" => "amenity/bbq.svg",
        "amenity_arts_centre" => "amenity/arts_centre.svg",
        "amenity_boat_rental" => "amenity/boat_rental.svg",
        "amenity_advertising_column" => "amenity/advertising_column.svg",
        "amenity_parking" => "amenity/parking.svg",
        "amenity_fountain" => "amenity/fountain.svg",
        "amenity_public_bookcase" => "amenity/public_bookcase.svg",
        "amenity_hunting_stand" => "amenity/hunting_stand.svg",
        "amenity_public_bath" => "amenity/public_bath.svg",
        "amenity_public_transport_tickets" => "amenity/public_transport_tickets.svg",
        /* ---- shop ---- */
        "shop_supermarket" => "shop/supermarket.svg",
        "shop_department_store" => "shop/department_store.svg",
        "shop_convenience" => "shop/convenience.svg",
        "shop_bakery" => "shop/bakery.svg",
        "shop_butcher" => "shop/butcher.svg",
        "shop_greengrocer" => "shop/greengrocer.svg",
        "shop_deli" => "shop/deli.svg",
        "shop_dairy" => "shop/dairy.svg",
        "shop_alcohol" => "shop/alcohol.svg",
        "shop_beverages" => "shop/beverages.svg",
        "shop_tea" => "shop/tea.svg",
        "shop_coffee" => "shop/coffee.svg",
        "shop_confectionery" => "shop/confectionery.svg",
        "shop_clothes" => "shop/clothes.svg",
        "shop_shoes" => "shop/shoes.svg",
        "shop_second_hand" => "shop/second_hand.svg",
        "shop_charity" => "shop/charity.svg",
        "shop_variety_store" => "shop/variety_store.svg",
        "shop_hairdresser" => "shop/hairdresser.svg",
        "shop_beauty" => "shop/beauty.svg",
        "shop_massage" => "shop/massage.svg",
        "shop_tattoo" => "shop/tattoo.svg",
        "shop_bicycle" => "shop/bicycle.svg",
        "shop_car" => "shop/car.svg",
        "shop_car_parts" => "shop/car_parts.svg",
        "shop_car_repair" => "shop/car_repair.svg",
        "shop_motorcycle" => "shop/motorcycle.svg",
        "shop_motorcycle_repair" => "shop/motorcycle_repair.svg",
        "shop_tyres" => "shop/tyres.svg",
        "shop_chemist" => "shop/chemist.svg",
        "shop_medical_supply" => "shop/medical_supply.svg",
        "shop_hearing_aids" => "shop/hearing_aids.svg",
        "shop_optician" => "shop/optician.svg",
        "shop_jewelry" => "shop/jewelry.svg",
        "shop_gift" => "shop/gift.svg",
        "shop_florist" => "shop/florist.svg",
        "shop_garden_centre" => "shop/garden_centre.svg",
        "shop_hardware" => "shop/hardware.svg",
        "shop_doityourself" => "shop/diy.svg",
        "shop_paint" => "shop/paint.svg",
        "shop_furniture" => "shop/furniture.svg",
        "shop_houseware" => "shop/houseware.svg",
        "shop_interior_decoration" => "shop/interior_decoration.svg",
        "shop_carpet" => "shop/carpet.svg",
        "shop_fabric" => "shop/fabric.svg",
        "shop_electronics" => "shop/electronics.svg",
        "shop_computer" => "shop/computer.svg",
        "shop_hifi" => "shop/hifi.svg",
        "shop_mobile_phone" => "shop/mobile_phone.svg",
        "shop_music" => "shop/music.svg",
        "shop_musical_instrument" => "shop/musical_instrument.svg",
        "shop_books" => "shop/stationery.svg",
        "shop_stationery" => "shop/stationery.svg",
        "shop_newsagent" => "shop/newsagent.svg",
        "shop_ticket" => "shop/ticket.svg",
        "shop_copyshop" => "shop/copyshop.svg",
        "shop_laundry" => "shop/laundry.svg",
        "shop_pet" => "shop/pet.svg",
        "shop_photo" => "shop/photo.svg",
        "shop_bag" => "shop/bag.svg",
        "shop_art" => "shop/art.svg",
        "shop_bed" => "shop/bed.svg",
        "shop_outdoor" => "shop/outdoor.svg",
        "shop_sports" => "shop/sports.svg",
        "shop_toys" => "shop/toys.svg",
        "shop_seafood" => "shop/seafood.svg",
        "shop_travel_agency" => "shop/travel_agency.svg",
        "shop_bookmaker" => "shop/bookmaker.svg",
        "shop_tobacco" => "shop/tobacco.svg",
        "shop_perfumery" => "shop/perfumery.svg",
        "shop_trade" => "shop/trade.svg",
        /* ---- tourism ---- */
        "tourism_hotel" => "tourism/hotel.svg",
        "tourism_motel" => "tourism/motel.svg",
        "tourism_guest_house" => "tourism/guest_house.svg",
        "tourism_hostel" => "tourism/hostel.svg",
        "tourism_apartment" => "tourism/apartment.svg",
        "tourism_chalet" => "tourism/chalet.svg",
        "tourism_alpine_hut" => "tourism/alpinehut.svg",
        "tourism_wilderness_hut" => "tourism/wilderness_hut.svg",
        "tourism_camp_site" => "tourism/camping.svg",
        "tourism_caravan_site" => "tourism/caravan_park.svg",
        "tourism_picnic_site" => "tourism/picnic.svg",
        "tourism_museum" => "tourism/museum.svg",
        "tourism_artwork" => "tourism/artwork.svg",
        "tourism_viewpoint" => "tourism/viewpoint.svg",
        "tourism_information" => "tourism/board.svg",
        "tourism_guidepost" => "tourism/guidepost.svg",
        "tourism_terminal" => "tourism/terminal.svg",
        "tourism_audioguide" => "tourism/audioguide.svg",
        "tourism_map" => "tourism/map.svg",
        /* ---- leisure ---- */
        "leisure_playground" => "leisure/playground.svg",
        "leisure_slipway" => "leisure/slipway.svg",
        "leisure_golf_course" => "leisure/golf.svg",
        "leisure_miniature_golf" => "leisure/miniature_golf.svg",
        "leisure_water_park" => "leisure/water_park.svg",
        "leisure_firepit" => "leisure/firepit.svg",
        "leisure_fitness_centre" => "leisure/fitness.svg",
        "leisure_fitness_station" => "leisure/fitness.svg",
        "leisure_amusement_arcade" => "leisure/amusement_arcade.svg",
        "leisure_beach_resort" => "leisure/beach_resort.svg",
        "leisure_bird_hide" => "leisure/bird_hide.svg",
        "leisure_bowling_alley" => "leisure/bowling_alley.svg",
        "leisure_dance" => "leisure/dance.svg",
        "leisure_fishing" => "leisure/fishing.svg",
        "leisure_sauna" => "leisure/sauna.svg",
        "leisure_outdoor_seating" => "leisure/outdoor_seating.svg",
        "leisure_picnic_table" => "tourism/picnic.svg",
        /* ---- healthcare ---- */
        "healthcare_hospital" => "amenity/hospital.svg",
        "healthcare_clinic" => "amenity/doctors.svg",
        "healthcare_pharmacy" => "amenity/pharmacy.svg",
        "healthcare_dentist" => "amenity/dentist.svg",
        "healthcare_doctor" => "amenity/doctors.svg",
        /* ---- point features with carto icons ---- */
        "@crossing" => "barrier/level_crossing.svg",
        "@tower" => "man_made/power_tower.svg",
        _ => return None,
    })
}

/// The POI zoom ladder (amenity-points.mss): major services from z14, most
/// retail/food from z15/16, low-priority furniture from z17.
pub fn poi_min_zoom(category: &str) -> u8 {
    match category {
        /* z14: major services */
        "amenity_hospital" | "amenity_place_of_worship" | "amenity_school"
        | "amenity_college" | "amenity_university" | "amenity_police"
        | "amenity_fire_station" | "amenity_bank" | "amenity_fuel"
        | "amenity_marketplace" | "amenity_townhall" | "amenity_courthouse"
        | "amenity_library" | "amenity_post_office" | "amenity_bus_station"
        | "amenity_ferry_terminal" | "amenity_aerodrome" | "amenity_prison"
        | "healthcare_hospital" | "leisure_stadium" | "tourism_museum" | "tourism_zoo"
        | "tourism_theme_park" | "shop_supermarket" | "amenity_pharmacy"
        | "amenity_clinic" | "healthcare_clinic" | "shop_department_store" => 14,
        /* z15: daily life */
        "amenity_cafe" | "amenity_restaurant" | "amenity_fast_food" | "amenity_bar"
        | "amenity_pub" | "amenity_biergarten" | "amenity_charging_station"
        | "shop_bakery" | "shop_butcher" | "shop_convenience" | "shop_greengrocer"
        | "tourism_hotel" | "tourism_guest_house" | "tourism_hostel" | "tourism_camp_site"
        | "tourism_information" | "tourism_attraction" | "tourism_viewpoint"
        | "leisure_playground" | "leisure_water_park" | "amenity_community_centre"
        | "amenity_car_wash" | "amenity_recycling" => 15,
        /* z17: low-priority furniture */
        "amenity_bench" | "amenity_atm" | "amenity_bicycle_parking"
        | "amenity_drinking_water" | "amenity_toilets" | "amenity_emergency_phone"
        | "leisure_picnic_table" | "leisure_firepit" | "shop_kiosk" | "shop_newsagent"
        | "amenity_post_box" | "shop_ticket" => 17,
        /* everything else z16 */
        _ => 16,
    }
}

/// Landcover pattern fills (landcover-area-symbols.mss): kind -> pattern svg.
/// Forest uses the leaf-type pattern set; cemetery the grave-yard patterns.
pub fn landcover_pattern(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "forest" => "leaftype_unknown.svg",
        "orchard" | "vineyard" => "patterns/orchard.svg",
        "plant_nursery" => "patterns/plant_nursery.svg",
        "allotments" => "patterns/allotments.svg",
        "quarry" => "quarry.svg",
        "flowerbed" => "flowerbed_mid_zoom.svg",
        "cemetery" | "grave_yard" => "patterns/grave_yard_generic.svg",
        "dog_park" => "patterns/dog_park.svg",
        _ => return None,
    })
}
