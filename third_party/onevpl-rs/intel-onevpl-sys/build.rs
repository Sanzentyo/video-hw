use std::{env, fs, path::PathBuf};

fn main() {
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let lib_vpl_include_path = env::var("LIBVPL_INCLUDE_PATH");

    let libvpl_include_path = match lib_vpl_include_path {
        Ok(path) => PathBuf::from(path),
        _ => {
            #[cfg(not(target_os = "windows"))]
            {
                // https://github.com/Intel-Media-SDK/MediaSDK/blob/master/api/include/mfxvideo.h
                // https://rust-lang.github.io/rust-bindgen/tutorial-3.html
                let libvpl = pkg_config::probe_library("vpl").unwrap();
                libvpl.include_paths[0].join("vpl")
            }
            #[cfg(target_os = "windows")]
            {
                if let Ok(oneapi_root) = env::var("ONEAPI_ROOT") {
                    PathBuf::from(oneapi_root)
                        .join("vpl")
                        .join("latest")
                        .join("include")
                        .join("vpl")
                } else {
                    // Force pregenerated fallback when include path is not explicitly configured.
                    PathBuf::from("vpl").join("include").join("vpl")
                }
            }
        }
    };
    let mfx_header = libvpl_include_path.join("mfx.h");

    if !mfx_header.exists() {
        let pregenerated = manifest_dir.join("src").join("bindings_pregenerated.rs");
        println!("cargo:rerun-if-changed={}", pregenerated.display());
        println!(
            "cargo:warning=LIBVPL header not found at {:?}; using pregenerated bindings at {:?}",
            mfx_header, pregenerated
        );
        fs::copy(&pregenerated, out_path.join("bindings.rs"))
            .expect("Couldn't copy pregenerated bindings");
        return;
    }

    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .header(mfx_header.to_string_lossy())
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .dynamic_library_name("vpl")
        .derive_debug(true)
        .impl_debug(true)
        // https://github.com/rust-lang/rust-bindgen/issues/2221
        .no_debug("mfx3DLutSystemBuffer")
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");

    // #[cfg(feature = "va")]
    // {
    //     println!("cargo:rustc-link-lib=dylib=va-drm");
    //     let libvadrm = pkg_config::probe_library("libva-drm").unwrap();
    //     let libvadrm_include_path = libvadrm.include_paths[0].join("va");
    //     let bindings = bindgen::Builder::default()
    //         .header(libvadrm_include_path.join("va_drm.h").to_string_lossy())
    //         .parse_callbacks(Box::new(bindgen::CargoCallbacks))
    //         .generate()
    //         .expect("Unable to generate bindings");

    //     bindings
    //         .write_to_file(out_path.join("bindings_va.rs"))
    //         .expect("Couldn't write bindings!");
    // }
}
