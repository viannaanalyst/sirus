//! The effort bar's moving fill, after Sirus Code's `.effort-slider`: a gradient, a nebula and a
//! flare near the thumb, drifting stars, rising embers and, with Fast, lightning.
use gpui::{
    Bounds, ContentMask, Corners, Hsla, PathBuilder, Pixels, Rgba, Window, fill, linear_color_stop,
    linear_gradient, point, px, size,
};

pub struct Energy {
    /// How much of the bar is filled, 0 to 1.
    pub fill: f32,
    pub tone: Hsla,
    pub soft: Hsla,
    pub cool: Hsla,
    /// Higher effort is hotter and faster, 0 to 1.
    pub heat: f32,
    pub embers: usize,
    pub fast: bool,
    /// Seconds; a constant stops the motion.
    pub time: f32,
}

pub fn blend(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

/// A wave going back and forth, like a CSS `alternate` animation.
fn sway(time: f32, period: f32) -> f32 {
    (time / period * std::f32::consts::PI).sin()
}

struct Area {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

/// A soft ellipse from stacked translucent quads, kept inside the filled part of the pill.
fn glow(
    window: &mut Window,
    area: &Area,
    center: (f32, f32),
    radius: (f32, f32),
    color: Hsla,
    peak: f32,
) {
    const LAYERS: usize = 12;
    for layer in 0..LAYERS {
        let k = 1. - layer as f32 / LAYERS as f32;
        let left = (center.0 - radius.0 * k).max(area.left);
        let right = (center.0 + radius.0 * k).min(area.right);
        let top = (center.1 - radius.1 * k).max(area.top);
        let bottom = (center.1 + radius.1 * k).min(area.bottom);
        if right <= left || bottom <= top {
            continue;
        }
        let shape = Bounds::new(
            point(px(left), px(top)),
            size(px(right - left), px(bottom - top)),
        );
        let corner = (right - left).min(bottom - top) / 2.;
        window.paint_quad(
            fill(
                shape,
                Hsla {
                    a: peak / LAYERS as f32,
                    ..color
                },
            )
            .corner_radii(px(corner)),
        );
    }
}

fn polyline(
    window: &mut Window,
    points: &[(f32, f32)],
    origin: (f32, f32),
    scale: (f32, f32),
    width: f32,
    color: Hsla,
) {
    let mut path = PathBuilder::stroke(px(width));
    for (index, (x, y)) in points.iter().enumerate() {
        let at = point(px(origin.0 + x * scale.0), px(origin.1 + y * scale.1));
        if index == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

pub fn paint(bounds: Bounds<Pixels>, energy: &Energy, window: &mut Window) {
    let (x, y) = (f32::from(bounds.left()), f32::from(bounds.top()));
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let filled = width * energy.fill;
    let round = height / 2.;
    let split = filled * 0.63;
    let speed = 1. - energy.heat * 0.6;
    let time = energy.time;
    let color = |hex: u32| -> Hsla { gpui::rgb(hex).into() };

    window.paint_quad(
        fill(
            Bounds::new(point(px(x), px(y)), size(px(split), px(height))),
            linear_gradient(
                90.,
                linear_color_stop(energy.tone, 0.19),
                linear_color_stop(energy.soft, 1.),
            ),
        )
        .corner_radii(Corners {
            top_left: px(round),
            bottom_left: px(round),
            top_right: px(0.),
            bottom_right: px(0.),
        }),
    );
    window.paint_quad(
        fill(
            Bounds::new(
                point(px(x + split), px(y)),
                size(px((filled - split).max(0.)), px(height)),
            ),
            linear_gradient(
                90.,
                linear_color_stop(energy.soft, 0.),
                linear_color_stop(energy.cool, 1.),
            ),
        )
        // Square: the thumb covers this end, so no dark notch shows beside it.
        .corner_radii(Corners {
            top_left: px(0.),
            bottom_left: px(0.),
            top_right: px(if energy.fill >= 1. { round } else { 0. }),
            bottom_right: px(if energy.fill >= 1. { round } else { 0. }),
        }),
    );

    let area = Area {
        left: x,
        right: x + filled,
        top: y,
        bottom: y + height,
    };
    let nebula_left = x + energy.fill * 0.55 * width - 90. + 18. * sway(time, 4.32 * speed);
    glow(
        window,
        &area,
        (nebula_left + 84., y + height * 0.4),
        (132., 26.),
        energy.cool,
        0.75,
    );
    glow(
        window,
        &area,
        (nebula_left + 136., y + height * 0.55),
        (128., 26.),
        energy.soft,
        0.75,
    );
    glow(
        window,
        &area,
        (nebula_left + 123., y + height * 0.49),
        (38., 16.),
        gpui::white(),
        0.9,
    );

    let flare = x + energy.fill * 0.68 * width + 14. * sway(time, 2.88);
    glow(
        window,
        &area,
        (flare, y + height / 2.),
        (20., height / 2.),
        gpui::white(),
        0.5 + 0.25 * sway(time, 2.88),
    );

    // Stars drift in a repeating strip, and leave the pill's round ends alone.
    let drift = (time / (2.88 * speed)).fract() * 160.;
    let stars: [(f32, f32, f32, f32); 4] = [
        (14., 10., 1.1, 1.),
        (48., 28., 1.6, 1.),
        (109., 13., 1.1, 0.8),
        (145., 27., 1.1, 1.),
    ];
    let mut strip = -160. + drift;
    while strip < filled {
        for (star_x, star_y, radius, alpha) in stars {
            let at = x + strip + star_x;
            let star_y = y + star_y * height / 38.;
            if at < x + 6. || at > x + filled - 2. {
                continue;
            }
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(px(at - radius), px(star_y - radius)),
                        size(px(radius * 2.), px(radius * 2.)),
                    ),
                    Hsla {
                        a: alpha,
                        ..gpui::white()
                    },
                )
                .corner_radii(px(radius)),
            );
        }
        strip += 160.;
    }

    for ember in 0..energy.embers {
        let phase = (time / 1.8 + ember as f32 * 0.19 / 1.8).fract();
        let at = x + width * (0.06 + ember as f32 * 0.09) + phase * 5.;
        if at > x + filled - 2. {
            continue;
        }
        let rise = y + height - 3.5 - phase * 24.;
        if rise < y {
            continue;
        }
        let alpha = if phase < 0.2 {
            phase / 0.2 * 0.8
        } else {
            0.8 * (1. - (phase - 0.2) / 0.8)
        };
        glow(
            window,
            &area,
            (at, rise),
            (4., 4.),
            color(0xffb547),
            alpha * 0.5,
        );
        window.paint_quad(
            fill(
                Bounds::new(
                    point(px(at - 1.25), px(rise - 1.25)),
                    size(px(2.5), px(2.5)),
                ),
                Hsla {
                    a: alpha,
                    ..color(0xffd59a)
                },
            )
            .corner_radii(px(1.25)),
        );
    }

    if energy.fast {
        let scale = (width / 360., height / 40.);
        let wobble = |index: usize| ((time * 9. + index as f32 * 1.7).sin()) * 1.6;
        let main: Vec<(f32, f32)> = [
            (0., 22.),
            (31., 12.),
            (45., 26.),
            (79., 10.),
            (95., 28.),
            (142., 15.),
            (130., 30.),
            (181., 9.),
            (202., 25.),
            (231., 13.),
            (251., 30.),
            (290., 10.),
            (308., 24.),
            (360., 13.),
        ]
        .iter()
        .enumerate()
        .map(|(index, (px_x, px_y))| (*px_x, *px_y + wobble(index)))
        .collect();
        let mask = ContentMask {
            bounds: Bounds::new(point(px(x), px(y)), size(px(filled), px(height))),
        };
        window.with_content_mask(Some(mask), |window| {
            polyline(
                window,
                &main,
                (x, y),
                scale,
                1.4,
                Hsla {
                    a: 0.85,
                    ..gpui::white()
                },
            );
        });
        // A bright strike sweeps the bar now and then.
        let strike = (time / 2.24).fract();
        if strike < 0.3 {
            let reach = ((strike / 0.18).min(1.) * 13.) as usize + 1;
            let alpha = if strike < 0.18 {
                1.
            } else {
                1. - (strike - 0.18) / 0.12
            };
            let bolt: [(f32, f32); 13] = [
                (0., 30.),
                (40., 14.),
                (62., 32.),
                (98., 10.),
                (120., 34.),
                (160., 12.),
                (176., 30.),
                (214., 8.),
                (236., 30.),
                (270., 14.),
                (300., 34.),
                (330., 16.),
                (360., 24.),
            ];
            polyline(
                window,
                &bolt[..reach.min(13)],
                (x, y - 4.),
                (width / 360., (height + 8.) / 48.),
                1.8,
                Hsla {
                    a: alpha,
                    ..gpui::white()
                },
            );
        }
    }
}
