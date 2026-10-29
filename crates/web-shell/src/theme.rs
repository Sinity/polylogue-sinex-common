//! Validated operator palette. Only hexadecimal RGB colours can reach CSS.
//! Semantic colours are fixed; an operator palette controls grounds and ink.
use serde::Deserialize;
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color([u8; 3]);
impl Color {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.strip_prefix('#').ok_or("colour must start with #")?;
        if value.len() != 6 || !value.is_ascii() {
            return Err("colour must have six hexadecimal digits".into());
        }
        let mut channels = [0; 3];
        for (index, channel) in channels.iter_mut().enumerate() {
            *channel = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
                .map_err(|_| "invalid colour")?;
        }
        Ok(Self(channels))
    }
    fn css(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2])
    }
    fn luminance(self) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.0[0]) + 0.7152 * channel(self.0[1]) + 0.0722 * channel(self.0[2])
    }
    pub fn contrast(self, other: Self) -> f64 {
        let a = self.luminance();
        let b = other.luminance();
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}
#[derive(Clone, Debug)]
pub struct Palette {
    ground: Color,
    surface: Color,
    sunk: Color,
    ink: Color,
}
impl Palette {
    pub fn new(ground: &str, surface: &str, sunk: &str, ink: &str) -> Result<Self, String> {
        let palette = Self {
            ground: Color::parse(ground)?,
            surface: Color::parse(surface)?,
            sunk: Color::parse(sunk)?,
            ink: Color::parse(ink)?,
        };
        for (name, color) in [
            ("ground", palette.ground),
            ("surface", palette.surface),
            ("surface-sunk", palette.sunk),
        ] {
            if palette.ink.contrast(color) < 4.5 {
                return Err(format!("ink on {name} has contrast below 4.5:1"));
            }
            // Semantic states remain legible through their designed contrasting backing.
        }
        palette.accents()?;
        Ok(palette)
    }
    pub fn pywal(json: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Wal {
            special: BTreeMap<String, String>,
            colors: BTreeMap<String, String>,
        }
        let wal: Wal = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let get = |map: &BTreeMap<String, String>, key: &str| {
            map.get(key)
                .cloned()
                .ok_or_else(|| format!("palette lacks {key}"))
        };
        Self::new(
            &get(&wal.special, "background")?,
            &get(&wal.colors, "color0")?,
            &get(&wal.special, "background")?,
            &get(&wal.special, "foreground")?,
        )
    }
    fn accents(&self) -> Result<[Color; 2], String> {
        let mut accents = [self.ink; 2];
        for (index, (name, candidates)) in [
            ("Polylogue", ["#455c9c", "#a2b8f4"]),
            ("Sinex", ["#2c6d57", "#8ed2b1"]),
        ]
        .into_iter()
        .enumerate()
        {
            accents[index] = candidates
                .into_iter()
                .map(|color| Color::parse(color).expect("constant colour"))
                .find(|color| {
                    [self.ground, self.surface, self.sunk]
                        .into_iter()
                        .all(|ground| color.contrast(ground) >= 4.5)
                })
                .ok_or_else(|| format!("{name} accent contrast is below 4.5:1"))?;
        }
        Ok(accents)
    }
    pub fn stylesheet(&self) -> String {
        let accents = self.accents().expect("palette was contrast validated");
        format!(
            ":root,:root[data-theme]{{--ground:{};--surface:{};--surface-sunk:{};--ink:{};--muted-ink:{};--line:{};--accent-polylogue:{};--accent-sinex:{}}}",
            self.ground.css(),
            self.surface.css(),
            self.sunk.css(),
            self.ink.css(),
            self.ink.css(),
            self.ink.css(),
            accents[0].css(),
            accents[1].css()
        )
    }
}
/// Failed reads and contrast checks are available to the product's theme view.
#[derive(Debug)]
pub struct Theme {
    pub stylesheet: Option<String>,
    pub fallback_reason: Option<String>,
}
impl Theme {
    pub fn load(path: Option<&std::path::Path>, enabled: bool) -> Self {
        if !enabled {
            return Self {
                stylesheet: None,
                fallback_reason: None,
            };
        }
        let default_path = std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".cache"))
            })
            .map(|cache| cache.join("wal/colors.json"));
        let path = path.or(default_path.as_deref());
        let Some(path) = path else {
            return Self {
                stylesheet: None,
                fallback_reason: None,
            };
        };
        if !path.exists() && default_path.as_deref() == Some(path) {
            return Self {
                stylesheet: None,
                fallback_reason: None,
            };
        }
        match std::fs::read_to_string(path)
            .map_err(|error| error.to_string())
            .and_then(|json| Palette::pywal(&json))
        {
            Ok(palette) => Self {
                stylesheet: Some(palette.stylesheet()),
                fallback_reason: None,
            },
            Err(reason) => Self {
                stylesheet: None,
                fallback_reason: Some(reason),
            },
        }
    }
}
