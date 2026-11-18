//! Fonts: skera subsets a TTF or OTF to the site's characters, and woofwoof writes WOFF2.

use anyhow::{Result, anyhow};
use read_fonts::FontRef;
use read_fonts::collections::IntSet;
use read_fonts::types::{GlyphId, NameId, Tag};
use skera::{DEFAULT_LAYOUT_FEATURES, Plan, SubsetFlags};

/// The fixed settings, which join the store key so a change invalidates old entries.
pub(super) const SETTINGS: &str = "default layout features, names 0 to 6, brotli 11";

/// Tables hb-subset drops by default. The layout tables browsers use stay.
const DROP: [&[u8; 4]; 17] = [
    b"morx", b"mort", b"kerx", b"kern", b"JSTF", b"DSIG", b"EBDT", b"EBLC", b"EBSC", b"SVG ",
    b"PCLT", b"LTSH", b"Feat", b"Glat", b"Gloc", b"Silf", b"Sill",
];

/// Subsets `font` to `chars` and returns WOFF2. The subset keeps hb-subset's default layout
/// features and the English name records 0 to 6, which hold the copyright line.
pub(super) fn subset(font: &[u8], chars: &str) -> Result<Vec<u8>> {
    let font = FontRef::new(font).map_err(|e| anyhow!("not a TrueType or OpenType font: {e}"))?;
    let mut names = IntSet::<NameId>::empty();
    names.insert_range(NameId::new(0)..=NameId::new(6));
    let plan = Plan::new(
        &IntSet::<GlyphId>::empty(),
        &chars.chars().map(u32::from).collect(),
        &font,
        SubsetFlags::default(),
        &DROP.into_iter().map(Tag::new).collect(),
        &IntSet::all(),
        &DEFAULT_LAYOUT_FEATURES.iter().copied().collect(),
        &names,
        &[0x0409u16].into_iter().collect(),
    );
    let ttf = skera::subset_font(&font, &plan).map_err(|e| anyhow!("subsetting failed: {e:?}"))?;
    woofwoof::compress(&ttf, "", 11, true).ok_or_else(|| anyhow!("WOFF2 encoding failed"))
}
