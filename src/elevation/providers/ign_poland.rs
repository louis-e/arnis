//! IGN Poland (NMT EVRF2007) elevation provider.
//!
//! Uses a **fixed global Web Mercator tile grid**.
//! Uses the Geoportal NMT WCS to fetch elevation data.

use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};

use super::fixed_tile::{
    fetch_fixed_tile_grid, FixedTileProvider, Resolution as ResolutionTrait, TileKey, TILE_PIXELS,
};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Resolution {
    /// 1.0 m/px — matches native resolution for NMT 1m.
    M1,
    /// ~3.44 m/px
    M3,
    /// ~10.31 m/px
    M10,
    /// ~30.92 m/px
    M30,
}

const LEVELS: &[Resolution] = &[
    Resolution::M1,
    Resolution::M3,
    Resolution::M10,
    Resolution::M30,
];

impl ResolutionTrait for Resolution {
    fn level_id(&self) -> &'static str {
        match self {
            Self::M1 => "r1",
            Self::M3 => "r3",
            Self::M10 => "r10",
            Self::M30 => "r30",
        }
    }

    fn meters_per_pixel(&self) -> f64 {
        match self {
            Self::M1 => 1.0,
            Self::M3 => 3.435_973_836_8,
            Self::M10 => 10.307_921_510_4,
            Self::M30 => 30.922_080_981_4,
        }
    }
}

pub struct IgnPoland;

impl FixedTileProvider for IgnPoland {
    type Level = Resolution;

    const CACHE_NAME: &'static str = "ign_poland_nmt";

    fn resolution_levels(&self) -> &'static [Self::Level] {
        LEVELS
    }

    fn tile_url(&self, key: &TileKey<Self::Level>) -> String {
        format!(
            "https://mapy.geoportal.gov.pl/wss/service/PZGIK/NMT/GRID1/WCS/DigitalTerrainModelFormatTIFF\
             ?SERVICE=WCS&VERSION=1.0.0&REQUEST=GetCoverage\
             &COVERAGE=DTM_PL-KRON86-NH_TIFF\
             &BBOX={:.6},{:.6},{:.6},{:.6}\
             &CRS=EPSG:3857&RESPONSE_CRS=EPSG:3857\
             &WIDTH={}&HEIGHT={}\
             &FORMAT=image/tiff&INTERPOLATION=bilinear",
            key.min_mx(),
            key.min_my(),
            key.max_mx(),
            key.max_my(),
            TILE_PIXELS,
            TILE_PIXELS,
        )
    }

    fn process_tile(&self, mut raster: Vec<Vec<f64>>) -> Result<Vec<Vec<f64>>, String> {
        let mut all_zero = true;
        let mut has_data = false;

        for row in &mut raster {
            for v in row.iter_mut() {
                if v.is_finite() {
                    has_data = true;
                    // Geoportal Poland sometimes returns valid TIFFs filled with 0.0 for tiles
                    // completely outside its coverage or missing data areas.
                    if *v != 0.0 {
                        all_zero = false;
                    } else {
                        *v = f64::NAN;
                    }
                }
            }
        }

        if !has_data {
            return Err("Tile contains no finite data".into());
        }

        if all_zero {
            return Err("Tile contains only 0.0 (interpreted as Geoportal Poland NoData)".into());
        }

        Ok(raster)
    }
}

impl ElevationProvider for IgnPoland {
    fn name(&self) -> &'static str {
        "ign_poland"
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        Some(vec![
            //North West: Szczecin, Koszalin, Gorzów Wielkopolski, Piła
            LLBBox::new(52.00, 14.12, 54.60, 18.00).unwrap(),

            //North Center: Gdańsk/Gdynia/Sopot (Trójmiasto), Olsztyn, Toruń, Bydgoszcz, Grudziądz, Elbląg
            LLBBox::new(52.00, 18.00, 54.84, 21.50).unwrap(),

            //North East: Białystok, Suwałki, Łomża, Ostrołęka
            LLBBox::new(52.00, 21.50, 54.40, 24.16).unwrap(),

            //South West: Wrocław, Poznań, Zielona Góra, Legnica, Jelenia Góra, Leszno, Kalisz
            LLBBox::new(49.60, 14.12, 52.00, 18.00).unwrap(),

            //South Center: Warszawa, Łódź, Kraków, Katowice (GOP / Upper Silesia), Częstochowa, Radom, Kielce, Opole
            LLBBox::new(49.15, 18.00, 52.00, 21.50).unwrap(),

            //South East: Lublin, Rzeszów, Tarnów, Przemyśl, Zamość, Chełm
            LLBBox::new(49.00, 21.50, 52.00, 24.16).unwrap(),
        ])
    }

    fn native_resolution_m(&self) -> f64 {
        1.0
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        fetch_fixed_tile_grid(self, bbox, grid_width, grid_height)
    }
}
