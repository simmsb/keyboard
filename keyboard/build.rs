use std::fs::File;
use std::io::Write;
use std::ops::Deref;
use std::path::PathBuf;
use std::env;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::Primitive,
    primitives::{Polyline, PrimitiveStyle},
};
use itertools::Itertools;
use stroke::{CubicBezier, PointN};
use svg::parser::Event;
use svgtypes::{PathParser, PathSegment};

fn point_as_tup(point: PointN<f64, 2>) -> (f64, f64) {
    let mut it = point.into_iter();
    let x = it.next().unwrap();
    let y = it.next().unwrap();
    (x, y)
}

fn process_paths(
    it: impl Iterator<Item = Result<PathSegment, svgtypes::Error>>,
) -> Vec<(f64, f64)> {
    let mut x_pos = 0.0;
    let mut y_pos = 0.0;
    let mut initial_x = 0.0;
    let mut initial_y = 0.0;

    let mut out = vec![];
    let mut emitted = false;

    for segment in it {
        let segment = segment.unwrap();

        eprintln!("segment ({x_pos}, {y_pos}): {:?}", segment);

        match segment {
            PathSegment::MoveTo { abs, x, y } => {
                if abs {
                    x_pos = x;
                    y_pos = y;
                } else {
                    x_pos += x;
                    y_pos += y;
                }

                initial_x = x_pos;
                initial_y = y_pos;

                emitted = false;
            }
            PathSegment::LineTo { abs, x, y } => {
                let (new_x, new_y) = if abs { (x, y) } else { (x_pos + x, y_pos + y) };
                if !emitted {
                    out.push((x_pos, y_pos));
                    emitted = true;
                }

                out.push((new_x, new_y));
                (x_pos, y_pos) = (new_x, new_y);
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                let (new_x, new_y) = if abs { (x, y_pos) } else { (x_pos + x, y_pos) };
                if !emitted {
                    out.push((x_pos, y_pos));
                    emitted = true;
                }

                out.push((new_x, new_y));
                (x_pos, y_pos) = (new_x, new_y);
            }
            PathSegment::VerticalLineTo { abs, y } => {
                let (new_x, new_y) = if abs { (x_pos, y) } else { (x_pos, y_pos + y) };
                if !emitted {
                    out.push((x_pos, y_pos));
                    emitted = true;
                }

                out.push((new_x, new_y));
                (x_pos, y_pos) = (new_x, new_y);
            }
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (new_x, new_y) = if abs { (x, y) } else { (x_pos + x, y_pos + y) };

                let (x1, y1) = if abs {
                    (x1, y1)
                } else {
                    (x_pos + x1, y_pos + y1)
                };

                let (x2, y2) = if abs {
                    (x2, y2)
                } else {
                    (x_pos + x2, y_pos + y2)
                };

                let curve = CubicBezier::new(
                    PointN::new([x_pos, y_pos]),
                    PointN::new([x1, y1]),
                    PointN::new([x2, y2]),
                    PointN::new([new_x, new_y]),
                );

                let length = curve.arclen(2);
                let segments = (length).max(2.0).min(6.0);
                let mut i = 0.0;

                while i <= 1.01 {
                    let next = curve.eval_casteljau(i);
                    eprintln!("   curve {i}: {next:?}");
                    out.push(point_as_tup(next));
                    i += 1.0 / segments;
                }
                (x_pos, y_pos) = point_as_tup(curve.eval_casteljau(1.0));
            }
            PathSegment::ClosePath { .. } => {
                out.push((initial_x, initial_y));
            }
            s => eprintln!("unhandled path segment type: {:?}", s),
        }
    }

    out
}

fn generate_image(paths: Vec<Vec<(f64, f64)>>) -> Vec<(i32, Vec<i32>)> {
    let dpi = 96.0;
    let ppmm = dpi / 25.4;

    let paths = paths
        .into_iter()
        .map(|path| {
            path.into_iter()
                .map(|(x, y)| {
                    embedded_graphics::prelude::Point::new((x * ppmm) as i32, (y * ppmm) as i32)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let pixels = paths
        .iter()
        .flat_map(|path| {
            let line =
                Polyline::new(path).into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2));

            line.pixels()
        })
        .filter_map(|pix| {
            if pix.1 == BinaryColor::On {
                Some(pix.0)
            } else {
                None
            }
        })
        .dedup()
        .filter(|pos| (0..32).contains(&pos.x))
        .filter(|pos| (0..128).contains(&pos.y))
        .sorted_by_key(|pos| pos.y)
        .group_by(|pos| pos.y);

    pixels
        .into_iter()
        .map(|(y, pixels)| (y, pixels.map(|pix| pix.x).collect::<Vec<_>>()))
        .collect::<Vec<_>>()
}

fn main() {
    // Put `memory.x` in our output directory and ensure it's
    // on the linker search path.
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=bongo/");

    for path in glob::glob("bongo/*.svg").unwrap() {
        let path = path.unwrap();
        let mut content = String::new();
        let svg = svg::open(&path, &mut content).unwrap();
        let paths = svg
            .filter_map(|e| {
                if let Event::Tag("path", _, attributes) = e {
                    eprintln!("path {:?}", attributes.get("id"));
                    attributes
                        .get("d")
                        .map(|v| process_paths(PathParser::from(v.deref())))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let image = generate_image(paths);

        let mut f = File::create(out.join(path.with_extension("rs").file_name().unwrap())).unwrap();

        write!(f, "&[").unwrap();
        for (y, row) in image {
            write!(f, "({}, &[", u8::try_from(y).unwrap()).unwrap();
            for x in row {
                write!(f, "{},", u8::try_from(x).unwrap()).unwrap()
            }
            write!(f, "]),").unwrap();
        }
        write!(f, "]").unwrap();

        eprintln!(
            "{:?}",
            out.join(path.with_extension("rs").file_name().unwrap())
        );
    }

    // panic!("lol");

    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("memory.x"))
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    // By default, Cargo will re-run a build script whenever
    // any file in the project changes. By specifying `memory.x`
    // here, we ensure the build script is only re-run when
    // `memory.x` is changed.
    println!("cargo:rerun-if-changed=memory.x");

    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
}
