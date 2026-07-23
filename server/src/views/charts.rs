//! LC-97: server-side inline-SVG charts for the admin analytics
//! dashboard. Rendering charts as SVG strings keeps the JS footprint at
//! zero (no chart.js / chartist island) and keeps the dashboard fully
//! server-rendered like the rest of the app. The output is a self-
//! contained `<svg>` element meant to be dropped into a template with
//! `|safe`; all interpolated values are numbers, so there is nothing to
//! escape.
//!
//! LC-570: the chart CHROME (gridlines, axis + date labels, empty-state
//! text) uses the palette tokens `var(--border)` / `var(--content-subtle)`
//! via inline `style` (CSS custom properties resolve in `style`, not in SVG
//! presentation attributes like `fill="..."`), so the axes recolor across
//! every palette and light/dark mode instead of the old hardcoded slate.
//! The per-series `color` stays a fixed categorical hue (one per metric, for
//! distinguishability), passed by the caller and read in both modes.

use crate::db::analytics::DayPoint;

/// Viewbox geometry. The SVG scales to its container via `width=100%`;
/// the viewBox fixes the internal coordinate space.
const W: f64 = 720.0;
const H: f64 = 180.0;
const PAD_L: f64 = 36.0;
const PAD_R: f64 = 8.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 22.0;

/// Render a filled line chart for a dense day series (gaps already filled
/// with zero by the caller). `color` is a CSS color (e.g. `#2563eb`).
/// Returns an empty-state `<svg>` with a centered "No data" label when
/// the series is empty so the template branch stays simple.
pub fn line_chart(points: &[DayPoint], color: &str) -> String {
    if points.is_empty() {
        return format!(
            "<svg viewBox=\"0 0 {W} {H}\" class=\"w-full h-auto\" role=\"img\" \
             aria-label=\"No data\"><text x=\"{cx}\" y=\"{cy}\" text-anchor=\"middle\" \
             style=\"fill:var(--content-subtle)\" font-size=\"12\">No data in range</text></svg>",
            cx = W / 2.0,
            cy = H / 2.0,
        );
    }

    let plot_w = W - PAD_L - PAD_R;
    let plot_h = H - PAD_T - PAD_B;
    let max = points.iter().map(|p| p.value).max().unwrap_or(0).max(1) as f64;
    let n = points.len();
    // With a single point, place it mid-width so it does not collapse to x=0.
    let dx = if n > 1 {
        plot_w / (n as f64 - 1.0)
    } else {
        0.0
    };

    let x_at = |i: usize| -> f64 {
        if n > 1 {
            PAD_L + dx * i as f64
        } else {
            PAD_L + plot_w / 2.0
        }
    };
    let y_at = |v: i64| -> f64 { PAD_T + plot_h * (1.0 - (v as f64 / max)) };

    let mut line = String::new();
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        line.push_str(&format!("{:.1},{:.1}", x_at(i), y_at(p.value)));
    }

    // Area path: down the line, then close along the baseline.
    let baseline = PAD_T + plot_h;
    let mut area = format!("M {:.1},{:.1}", x_at(0), baseline);
    for (i, p) in points.iter().enumerate() {
        area.push_str(&format!(" L {:.1},{:.1}", x_at(i), y_at(p.value)));
    }
    area.push_str(&format!(" L {:.1},{:.1} Z", x_at(n - 1), baseline));

    // Y axis gridlines at 0, half, max.
    let mut grid = String::new();
    for frac in [0.0_f64, 0.5, 1.0] {
        let y = PAD_T + plot_h * (1.0 - frac);
        let label = (max * frac).round() as i64;
        grid.push_str(&format!(
            "<line x1=\"{PAD_L}\" y1=\"{y:.1}\" x2=\"{x2:.1}\" y2=\"{y:.1}\" \
             style=\"stroke:var(--border)\" stroke-width=\"1\"/>\
             <text x=\"{tx:.1}\" y=\"{ty:.1}\" text-anchor=\"end\" style=\"fill:var(--content-subtle)\" \
             font-size=\"10\">{label}</text>",
            x2 = W - PAD_R,
            tx = PAD_L - 4.0,
            ty = y + 3.0,
        ));
    }

    // X axis end labels (first and last date) to give the range context.
    let first_date = &points[0].date;
    let last_date = &points[n - 1].date;

    format!(
        "<svg viewBox=\"0 0 {W} {H}\" class=\"w-full h-auto\" role=\"img\" \
         aria-label=\"line chart\">\
         {grid}\
         <path d=\"{area}\" fill=\"{color}\" fill-opacity=\"0.12\"/>\
         <polyline points=\"{line}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\" \
         stroke-linejoin=\"round\" stroke-linecap=\"round\"/>\
         <circle cx=\"{lx:.1}\" cy=\"{ly:.1}\" r=\"3\" fill=\"{color}\"/>\
         <text x=\"{PAD_L}\" y=\"{tby:.1}\" style=\"fill:var(--content-subtle)\" font-size=\"10\">{first_date}</text>\
         <text x=\"{rx:.1}\" y=\"{tby:.1}\" text-anchor=\"end\" style=\"fill:var(--content-subtle)\" \
         font-size=\"10\">{last_date}</text>\
         </svg>",
        lx = x_at(n - 1),
        ly = y_at(points[n - 1].value),
        rx = W - PAD_R,
        tby = H - 6.0,
    )
}
