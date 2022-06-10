use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use image::{DynamicImage, GenericImageView, Rgba};
use itertools::Itertools;

fn generate_image(image: DynamicImage) -> Vec<(u32, Vec<(u32, bool)>)> {
    let pixels = image
        .pixels()
        .filter_map(|(x, y, c)| {
            println!("{} {} {:?}", x, y, c);
            match c {
            Rgba([0, 0, 0, 255]) => Some((x, y, true)),
            Rgba([255, 255, 255, 255]) => Some((x, y, false)),
            _ => None,
        }})
        .sorted_by_key(|(_, y, _)| *y)
        .group_by(|(_, y, _)| *y);

    pixels
        .into_iter()
        .map(|(y, pixels)| (y, pixels.map(|(x, _, v)| (x, v)).collect::<Vec<_>>()))
        .collect::<Vec<_>>()
}

fn main() {
    // Put `memory.x` in our output directory and ensure it's
    // on the linker search path.
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=bongo/");

    for path in glob::glob("bongo/*.png").unwrap() {
        let path = path.unwrap();
        let image = image::io::Reader::open(&path).unwrap().decode().unwrap();
        let image = generate_image(image);

        let mut f = File::create(out.join(path.with_extension("rs").file_name().unwrap())).unwrap();

        write!(f, "&[").unwrap();
        for (y, row) in image {
            write!(f, "({}, &[", u8::try_from(y).unwrap()).unwrap();
            for (x, on) in row {
                write!(f, "({}, {}),", u8::try_from(x).unwrap(), on).unwrap()
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
