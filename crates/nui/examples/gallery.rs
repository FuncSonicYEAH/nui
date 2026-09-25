//! Gallery demo (M8 acceptance): `Image` elements (async decode, tint,
//! nine-slice) over a rect with a soft SDF shadow.
//!
//! Run: `cargo run -p nui --example gallery`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::Size;

/// Writes a 64x64 24-bit BMP: a blue-to-orange gradient disc on a
/// light checker, used as the demo texture (no encoder dependency needed).
fn write_demo_bmp(path: &str) {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;
    let row_padding = (4 - (WIDTH * 3) % 4) % 4;
    let row_size = WIDTH * 3 + row_padding;
    let data_size = 54 + row_size * HEIGHT;
    let mut bytes: Vec<u8> = Vec::with_capacity(data_size as usize);
    // BITMAPFILEHEADER
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&WIDTH.to_le_bytes());
    bytes.extend_from_slice(&HEIGHT.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&24u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(row_size * HEIGHT).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 16]);
    // Pixels, bottom-up rows, BGR order.
    for y in (0..HEIGHT).rev() {
        for x in 0..WIDTH {
            let dx = x as f32 - 32.0;
            let dy = y as f32 - 32.0;
            let inside = dx * dx + dy * dy <= 26.0 * 26.0;
            let (r, g, b) = if inside {
                let t = x as f32 / 64.0;
                (
                    (255.0 * (1.0 - t)) as u8,
                    (140.0 * t) as u8,
                    (60.0 + 160.0 * t) as u8,
                )
            } else {
                let checker = (x / 8 + y / 8) % 2 == 0;
                if checker {
                    (233, 236, 240)
                } else {
                    (205, 210, 218)
                }
            };
            bytes.extend_from_slice(&[b, g, r]);
        }
        bytes.extend(std::iter::repeat_n(0u8, row_padding as usize));
    }
    std::fs::write(path, bytes).expect("demo texture write works");
}

fn main() {
    let texture_path = std::env::temp_dir().join("nui-gallery-demo.bmp");
    write_demo_bmp(texture_path.to_str().unwrap_or("gallery.bmp"));
    let source = texture_path.display().to_string();

    let document = format!(
        r#"
component Gallery {{
    Window(id = root) {{
        Column(id = content, spacing = 20dp, padding = 28dp) {{
            Rectangle(
                id = card,
                width = 300dp, height = 120dp, radius = 12dp,
                fill = #2b3240,
                shadow.dx = 0dp, shadow.dy = 10dp,
                shadow.blur = 16dp, shadow.color = #000000b0,
            )
            Image(
                id = picture,
                source = "{source}",
                width = 300dp, height = 120dp,
                slice = 20dp, tint = #ffffff,
            )
        }}
    }}
}}
"#
    );

    let config = AppConfig::new(document, "nui — gallery", Size::new(420.0, 380.0));
    Application::new(config).run();
}
