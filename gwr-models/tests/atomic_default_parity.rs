// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Independent old-API observations recorded before/after foundation adoption.
#[test]
#[ignore = "requires separately recorded immutable before and after directories"]
fn actual_ordinary_cache_and_platform_none_bytes_remain_exact() {
    let before = std::path::PathBuf::from(
        std::env::var_os("GWR_ATOMIC_DEFAULT_BEFORE").expect("before root"),
    );
    let after =
        std::path::PathBuf::from(std::env::var_os("GWR_ATOMIC_DEFAULT_AFTER").expect("after root"));
    assert!(before.is_absolute() && after.is_absolute());
    assert_ne!(
        before.canonicalize().unwrap(),
        after.canonicalize().unwrap()
    );
    for name in [
        "ordinary-memory.yaml",
        "native-observations.yaml",
        "canonical-platform.yaml",
    ] {
        let a = before.join(name);
        let b = after.join(name);
        assert!(a.symlink_metadata().unwrap().file_type().is_file());
        assert!(b.symlink_metadata().unwrap().file_type().is_file());
        let expected = std::fs::read(a).unwrap();
        assert!(!expected.is_empty());
        assert_eq!(
            std::fs::read(b).unwrap(),
            expected,
            "independent old-API bytes: {name}"
        );
    }
}
