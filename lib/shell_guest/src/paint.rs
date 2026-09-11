//! Small, readable shell text without depending on the demo glyph placeholders.
use bexos_dioxus_guest::scene::{Command, Paint, PathCommand, Point, SceneBatch};
pub const INK: [u8; 4] = [232, 238, 248, 255];
pub fn scene(width: u32, height: u32, color: [u8; 4]) -> SceneBatch {
    SceneBatch {
        width,
        height,
        clear_rgba: color,
        commands: Vec::new(),
    }
}
pub fn rect(s: &mut SceneBatch, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
    s.commands.push(Command::Path(PathCommand {
        points: vec![
            Point { x, y },
            Point { x: x + w, y },
            Point { x: x + w, y: y + h },
            Point { x, y: y + h },
        ],
        closed: true,
        paint: Paint::Solid(color),
        stroke_width: 0.0,
    }));
}
// Original 5-column glyphs, top bit first. Lowercase uses the same clear capitals.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 6, 6],
        ':' => [0, 6, 6, 0, 6, 6, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        '*' => [0, 21, 14, 31, 14, 21, 0],
        '>' => [16, 8, 4, 2, 4, 8, 16],
        '<' => [1, 2, 4, 8, 4, 2, 1],
        ' ' => [0; 7],
        _ => [14, 17, 1, 2, 4, 0, 4],
    }
}
pub fn text(s: &mut SceneBatch, x: f32, y: f32, value: &str, scale: f32, color: [u8; 4]) {
    for (i, c) in value.chars().take(64).enumerate() {
        for (row, bits) in glyph(c).into_iter().enumerate() {
            // Merge adjacent horizontal pixels to keep the scene command count bounded.
            let mut col = 0;
            while col < 5 {
                if bits & (16 >> col) == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < 5 && bits & (16 >> col) != 0 {
                    col += 1;
                }
                rect(
                    s,
                    x + i as f32 * 6.0 * scale + start as f32 * scale,
                    y + row as f32 * scale,
                    (col - start) as f32 * scale,
                    scale,
                    color,
                );
            }
        }
    }
}
pub fn button(s: &mut SceneBatch, x: f32, y: f32, w: f32, label: &str, selected: bool) {
    rect(
        s,
        x,
        y,
        w,
        32.0,
        if selected {
            [56, 103, 183, 255]
        } else {
            [48, 60, 80, 255]
        },
    );
    text(s, x + 10.0, y + 9.0, label, 2.0, INK);
}
pub fn inside(x: f64, y: f64, rect: (f32, f32, f32, f32)) -> bool {
    x >= rect.0 as f64
        && y >= rect.1 as f64
        && x < (rect.0 + rect.2) as f64
        && y < (rect.1 + rect.3) as f64
}
