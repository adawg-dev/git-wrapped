use resvg::{tiny_skia, usvg};

const MAX_PIXELS: u64 = 64_000_000;
const MAX_SCALE: u32 = 4;
const FONT: &[u8] = include_bytes!("../../assets/Lato-Regular.ttf");

fn tree(svg: &str) -> Result<usvg::Tree, String> {
    let mut options = usvg::Options {
        font_family: "Lato".into(),
        ..usvg::Options::default()
    };
    options.fontdb_mut().load_font_data(FONT.to_vec());
    usvg::Tree::from_str(svg, &options).map_err(|e| format!("parse SVG: {e}"))
}

fn checked_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("PNG dimensions exceed the 64 million pixel limit".into());
    }
    Ok(())
}

fn render_pixmap(tree: &usvg::Tree, width: u32, height: u32) -> Result<tiny_skia::Pixmap, String> {
    checked_dimensions(width, height)?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("PNG dimensions too large")?;
    let transform = tiny_skia::Transform::from_scale(
        width as f32 / tree.size().width(),
        height as f32 / tree.size().height(),
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}

pub(crate) fn rasterize(svg: &str, scale: u32) -> Result<Vec<u8>, String> {
    if !(1..=MAX_SCALE).contains(&scale) {
        return Err(format!("PNG scale must be between 1 and {MAX_SCALE}"));
    }
    let tree = tree(svg)?;
    let width = tree.size().width().ceil();
    let height = tree.size().height().ceil();
    if width > u32::MAX as f32 || height > u32::MAX as f32 {
        return Err("PNG dimensions too large".into());
    }
    let width = (width as u32)
        .checked_mul(scale)
        .ok_or("PNG dimensions too large")?;
    let height = (height as u32)
        .checked_mul(scale)
        .ok_or("PNG dimensions too large")?;
    render_pixmap(&tree, width, height)?
        .encode_png()
        .map_err(|e| format!("encode PNG: {e}"))
}

pub fn render_rgba(svg: &str, width: u32, height: u32) -> Result<Vec<u8>, String> {
    Ok(render_pixmap(&tree(svg)?, width, height)?.data().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_and_scale_are_bounded() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="10000"/>"#;
        assert!(rasterize(svg, 1).unwrap_err().contains("64 million"));
        let small = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
        assert!(rasterize(small, 0).is_err());
        assert!(rasterize(small, 5).is_err());
        assert!(render_rgba(small, 8001, 8000).is_err());
    }

    #[test]
    fn bundled_font_draws_text_without_system_fonts() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><text x="5" y="30" font-size="24" font-family="Lato">Hi</text></svg>"#;
        let rgba = render_rgba(svg, 100, 50).unwrap();
        assert!(rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0));
    }
}
