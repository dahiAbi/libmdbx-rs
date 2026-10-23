use bindgen::{
    Formatter,
    callbacks::{IntKind, ParseCallbacks},
};
use std::{
    env,
    path::{Path, PathBuf},
};

const LIBMDBX_DIR: &str = "vendor/libmdbx";

const MDBX_CONSTANTS: &[&str] = &[
    "MDBX_SUCCESS",
    "MDBX_KEYEXIST",
    "MDBX_NOTFOUND",
    "MDBX_PAGE_NOTFOUND",
    "MDBX_CORRUPTED",
    "MDBX_PANIC",
    "MDBX_VERSION_MISMATCH",
    "MDBX_INVALID",
    "MDBX_MAP_FULL",
    "MDBX_DBS_FULL",
    "MDBX_READERS_FULL",
    "MDBX_TLS_FULL",
    "MDBX_TXN_FULL",
    "MDBX_CURSOR_FULL",
    "MDBX_PAGE_FULL",
    "MDBX_MAP_RESIZED",
    "MDBX_INCOMPATIBLE",
    "MDBX_BAD_RSLOT",
    "MDBX_BAD_TXN",
    "MDBX_BAD_VALSIZE",
    "MDBX_BAD_DBI",
    "MDBX_LOG_DONTCHANGE",
    "MDBX_DBG_DONTCHANGE",
    "MDBX_RESULT_TRUE",
    "MDBX_UNABLE_EXTEND_MAPSIZE",
    "MDBX_PROBLEM",
    "MDBX_LAST_LMDB_ERRCODE",
    "MDBX_BUSY",
    "MDBX_EMULTIVAL",
    "MDBX_EBADSIGN",
    "MDBX_WANNA_RECOVERY",
    "MDBX_EKEYMISMATCH",
    "MDBX_TOO_LARGE",
    "MDBX_THREAD_MISMATCH",
    "MDBX_TXN_OVERLAPPING",
    "MDBX_BACKLOG_DEPLETED",
    "MDBX_DUPLICATED_CLK",
    "MDBX_DANGLING_DBI",
    "MDBX_OUSTED",
    "MDBX_MVCC_RETARDED",
    "MDBX_LAST_ERRCODE",
];

#[derive(Debug)]
struct Callbacks;

impl ParseCallbacks for Callbacks {
    fn int_macro(&self, name: &str, _value: i64) -> Option<IntKind> {
        if MDBX_CONSTANTS.contains(&name) {
            Some(IntKind::Int)
        } else {
            Some(IntKind::UInt)
        }
    }
}

fn is_android(target: &str) -> bool {
    target.ends_with("-linux-android") || target.ends_with("-linux-androideabi")
}

/// Clang target triple prefix used by the NDK wrapper scripts.
/// Note: armv7 uses `armv7a`, not `armv7`.
fn android_clang_triple(target: &str) -> &'static str {
    match target {
        "aarch64-linux-android" => "aarch64-linux-android",
        "armv7-linux-androideabi" => "armv7a-linux-androideabi",
        "x86_64-linux-android" => "x86_64-linux-android",
        "i686-linux-android" => "i686-linux-android",
        _ => panic!("Unsupported Android target: {target}"),
    }
}

fn android_api_level() -> String {
    env::var("ANDROID_API_LEVEL").unwrap_or_else(|_| "23".to_owned())
}

fn find_android_ndk() -> PathBuf {
    if let Some(p) = env::var_os("ANDROID_NDK_HOME").or_else(|| env::var_os("ANDROID_NDK_ROOT")) {
        return PathBuf::from(p);
    }

    let sdk = env::var_os("ANDROID_SDK_ROOT")
        .or_else(|| env::var_os("ANDROID_HOME"))
        .map(PathBuf::from)
        .expect("Set ANDROID_NDK_HOME, ANDROID_NDK_ROOT, or ANDROID_SDK_ROOT");

    std::fs::read_dir(sdk.join("ndk"))
        .unwrap_or_else(|e| panic!("Cannot read {}/ndk: {e}", sdk.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .max()
        .expect("No NDK versions found in $ANDROID_SDK_ROOT/ndk")
}

fn ndk_host_tag() -> &'static str {
    if cfg!(target_os = "macos") {
        // The NDK ships a universal binary under darwin-x86_64, also for arm64 hosts.
        "darwin-x86_64"
    } else if cfg!(target_os = "windows") {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    }
}

fn ndk_llvm_dir(ndk: &Path) -> PathBuf {
    ndk.join("toolchains/llvm/prebuilt").join(ndk_host_tag())
}

fn build_libmdbx(source: &Path) {
    let target = env::var("TARGET").expect("TARGET");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS");

    let mut build = cc::Build::new();

    build
        .file(source.join("mdbx.c"))
        .include(source)
        .opt_level(3)
        .warnings(false)
        .define("NDEBUG", None)
        .define("MDBX_BUILD_SHARED_LIBRARY", "0")
        .define("MDBX_TXN_CHECKOWNER", "0");

    if !target.contains("windows") {
        build.pic(true);
    }

    // The CMake build probed for this and disabled it where it doesn't link.
    // __builtin_cpu_supports needs __cpu_model / __cpu_indicator_init from
    // libgcc/compiler-rt, which aren't available (or can't be linked into a PIC
    // shared object) on x86 Android and MSVC cross builds.
    build.define("MDBX_HAVE_BUILTIN_CPU_SUPPORTS", "0");

    if target_os == "android" {
        // Pick the NDK's clang wrapper explicitly so ANDROID_API_LEVEL is honored.
        let ndk = find_android_ndk();
        let api = android_api_level();
        let bin = ndk_llvm_dir(&ndk).join("bin");
        let clang = bin.join(format!("{}{api}-clang", android_clang_triple(&target)));
        let ar = bin.join("llvm-ar");

        assert!(clang.is_file(), "NDK clang not found: {}", clang.display());

        build.compiler(clang);
        if ar.is_file() {
            build.archiver(ar);
        }
    }

    // Compiles mdbx.c and emits:
    //   cargo:rustc-link-search=native=$OUT_DIR
    //   cargo:rustc-link-lib=static=mdbx
    build.compile("mdbx");
}

fn generate_bindings() {
    let target = env::var("TARGET").expect("TARGET is not set");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is not set"));

    let header = Path::new(LIBMDBX_DIR).join("mdbx.h");

    let mut builder = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .allowlist_var("^(MDBX|mdbx)_.*")
        .allowlist_type("^(MDBX|mdbx)_.*")
        .allowlist_function("^(MDBX|mdbx)_.*")
        .size_t_is_usize(true)
        .ctypes_prefix("::libc")
        .parse_callbacks(Box::new(Callbacks))
        .layout_tests(false)
        .prepend_enum_name(false)
        .generate_comments(false)
        .disable_header_comment()
        .formatter(Formatter::None);

    if is_android(&target) {
        let ndk = find_android_ndk();
        let api = android_api_level();
        let sysroot = ndk_llvm_dir(&ndk).join("sysroot");

        assert!(
            sysroot.is_dir(),
            "NDK sysroot not found: {}",
            sysroot.display()
        );

        builder = builder
            .clang_arg(format!("--target={target}{api}"))
            .clang_arg(format!("--sysroot={}", sysroot.display()));
    }

    let bindings = builder
        .generate()
        .expect("Failed to generate libmdbx bindings");

    let output = out_dir.join("bindings.rs");

    bindings
        .write_to_file(&output)
        .unwrap_or_else(|e| panic!("Failed to write {}: {e}", output.display()));
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={LIBMDBX_DIR}/mdbx.h");
    println!("cargo:rerun-if-changed={LIBMDBX_DIR}/mdbx.c");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_HOME");
    println!("cargo:rerun-if-env-changed=ANDROID_NDK_ROOT");
    println!("cargo:rerun-if-env-changed=ANDROID_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=ANDROID_HOME");
    println!("cargo:rerun-if-env-changed=ANDROID_API_LEVEL");

    let source = PathBuf::from(LIBMDBX_DIR);

    assert!(
        source.join("mdbx.c").is_file() && source.join("mdbx.h").is_file(),
        "Expected the amalgamated MDBX sources mdbx.c and mdbx.h in {LIBMDBX_DIR}. \
         Run: git submodule update --init --recursive"
    );

    generate_bindings();
    build_libmdbx(&source);

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS");

    match target_os.as_str() {
        // "android" => println!("cargo:rustc-link-lib=log"),
        "windows" => {
            println!("cargo:rustc-link-lib=user32"); // CharToOemBuffA
            println!("cargo:rustc-link-lib=advapi32");
            println!("cargo:rustc-link-lib=ntdll");
        }
        _ => {}
    }
}
