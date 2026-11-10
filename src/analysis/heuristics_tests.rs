use super::*;

fn verdict(name: &str, args: &[&str]) -> Verdict {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    assess(name, &args)
}

fn label(name: &str, args: &[&str]) -> Option<(&'static str, Severity)> {
    match verdict(name, args) {
        Verdict::Flag(a) => Some((a.label, a.severity)),
        _ => None,
    }
}

#[test]
fn non_literal_format_strings_are_flagged() {
    assert_eq!(
        label("printf", &["buf"]),
        Some(("non-literal format", Severity::High))
    );
    assert_eq!(
        label("fprintf", &["stderr", "msg"]),
        Some(("non-literal format", Severity::High))
    );
    assert_eq!(
        label("snprintf", &["d", "sizeof d", "user"]),
        Some(("non-literal format", Severity::High))
    );
    assert_eq!(verdict("printf", &["\"%s\\n\"", "buf"]), Verdict::Default);
    assert_eq!(verdict("printf", &["\"%\" PRIu64", "n"]), Verdict::Default);
    assert_eq!(
        verdict("printf", &["_(\"Hello %s\")", "n"]),
        Verdict::Default
    );
    assert_eq!(verdict("printf", &[]), Verdict::Default);
}

#[test]
fn sprintf_is_judged_by_its_conversions() {
    assert_eq!(
        label("sprintf", &["d", "\"name=%s\"", "n"]),
        Some(("unbounded %s", Severity::High))
    );
    assert_eq!(
        label("sprintf", &["d", "\"%d-%08x\"", "a", "b"]),
        Some(("fixed-width conversions", Severity::Low))
    );
    assert_eq!(
        label("sprintf", &["d", "\"%f\"", "x"]),
        Some(("long conversions", Severity::Medium))
    );
    assert_eq!(
        label("sprintf", &["d", "\"%*d\"", "w", "x"]),
        Some(("long conversions", Severity::Medium))
    );
    for unbounded in ["\"%1$s\"", "\"%S\"", "\"%ws\"", "\"%I64d %s\""] {
        assert_eq!(
            label("sprintf", &["d", unbounded, "x"]),
            Some(("unbounded %s", Severity::High)),
            "{unbounded}"
        );
    }
    // A precision bounds %s.
    assert_eq!(
        label("sprintf", &["d", "\"%.16s\"", "n"]),
        Some(("fixed-width conversions", Severity::Low))
    );
}

#[test]
fn scanf_without_unbounded_strings_is_not_flagged() {
    assert_eq!(
        label("sscanf", &["s", "\"%s %d\"", "buf", "&n"]),
        Some(("unbounded %s", Severity::High))
    );
    assert_eq!(
        label("scanf", &["\"%[^\\n]\"", "line"]),
        Some(("unbounded %s", Severity::High))
    );
    assert_eq!(
        label("sscanf", &["s", "\"%63s\"", "buf"]),
        Some(("string widths", Severity::Low))
    );
    assert_eq!(
        verdict("sscanf", &["s", "\"%llu%c\"", "&n", "&c"]),
        Verdict::Suppress
    );
    // Assignment-suppressed conversions write nothing.
    assert_eq!(
        verdict("sscanf", &["s", "\"%*s %d\"", "&n"]),
        Verdict::Suppress
    );
}

#[test]
fn copies_sized_by_their_source_are_flagged() {
    assert_eq!(
        label("memcpy", &["dst", "src", "strlen(src)"]),
        Some(("size from the source", Severity::High))
    );
    assert_eq!(
        label("strncpy", &["d", "s", "strlen( s ) + 1"]),
        Some(("size from the source", Severity::High))
    );
    assert_eq!(verdict("memcpy", &["dst", "src", "n"]), Verdict::Default);
    assert_eq!(
        label("memcpy", &["dst", "src", "16"]),
        Some(("constant size", Severity::Low))
    );
    assert_eq!(
        verdict("memcpy", &["dst", "src", "sizeof(hdr)"]),
        Verdict::Default
    );
}

#[test]
fn fixed_strings_and_commands_are_downgraded() {
    assert_eq!(
        label("strcpy", &["d", "\"none\""]),
        Some(("fixed string", Severity::Low))
    );
    assert_eq!(verdict("strcpy", &["d", "s"]), Verdict::Default);
    assert_eq!(
        label("system", &["\"clear\""]),
        Some(("constant command", Severity::Low))
    );
    assert_eq!(verdict("system", &["cmd"]), Verdict::Default);
}

#[test]
fn allocation_sizes_that_can_overflow_are_flagged() {
    assert_eq!(
        label("malloc", &["n * sizeof(struct item)"]),
        Some(("size can overflow", Severity::Medium))
    );
    assert_eq!(
        label("realloc", &["p", "count*size"]),
        Some(("size can overflow", Severity::Medium))
    );
    assert_eq!(verdict("malloc", &["4 * sizeof(int)"]), Verdict::Default);
    // Multiplying by one can't overflow.
    assert_eq!(
        verdict("malloc", &["sizeof(char) * str.size()"]),
        Verdict::Default
    );
    assert_eq!(verdict("malloc", &["1 * n"]), Verdict::Default);
    assert_eq!(verdict("malloc", &["n"]), Verdict::Default);
    assert_eq!(
        label("alloca", &["len"]),
        Some(("variable size", Severity::High))
    );
    assert_eq!(
        label("alloca", &["64"]),
        Some(("constant size", Severity::Low))
    );
    assert_eq!(
        label("alloca", &["sizeof x"]),
        Some(("constant size", Severity::Low))
    );
    assert_eq!(
        label("malloc", &["((a * b))"]),
        Some(("size can overflow", Severity::Medium))
    );
    assert_eq!(
        label("malloc", &["n * *p"]),
        Some(("size can overflow", Severity::Medium))
    );
    assert_eq!(verdict("malloc", &["*len"]), Verdict::Default);
}

#[test]
fn literals_joined_to_macros_are_literal_formats() {
    let printk = |format: &str| verdict("printk", &[format, "x"]);
    assert_eq!(printk("KERN_INFO \"probe %d\\n\""), Verdict::Default);
    assert_eq!(
        printk("KERN_ERR PFX \"bad \\\"%s\\\"\\n\""),
        Verdict::Default
    );
    assert_eq!(
        label("printk", &["msg"]),
        Some(("non-literal format", Severity::High))
    );
    assert_eq!(
        label("printk", &["KERN_INFO msg"]),
        Some(("non-literal format", Severity::High))
    );
    assert_eq!(
        label("printk", &["KERN_INFO \"unclosed"]),
        Some(("non-literal format", Severity::High))
    );
    assert_eq!(
        label("sprintf", &["d", "DRV_NAME \": %s\"", "x"]),
        Some(("unbounded %s", Severity::High))
    );
}

#[test]
fn kernel_and_windows_allocators_are_checked_for_overflow() {
    for (name, args) in [
        ("kmalloc", &["n * size", "GFP_KERNEL"][..]),
        ("devm_kzalloc", &["dev", "count * sizeof(*p)", "GFP_KERNEL"]),
        ("HeapAlloc", &["heap", "0", "n * m"]),
    ] {
        assert_eq!(
            label(name, args),
            Some(("size can overflow", Severity::Medium)),
            "{name}"
        );
    }
    assert_eq!(
        verdict("kmalloc", &["sizeof(*p)", "GFP_KERNEL"]),
        Verdict::Default
    );
}

#[test]
fn realpath_without_a_buffer_is_safe() {
    assert_eq!(verdict("realpath", &["path", "NULL"]), Verdict::Suppress);
    assert_eq!(verdict("realpath", &["path", "buf"]), Verdict::Default);
}

#[test]
fn turning_off_certificate_checks_is_flagged_and_nothing_else() {
    let off = Some(("certificate check off", Severity::High));
    assert_eq!(
        label("SSL_CTX_set_verify", &["ctx", "SSL_VERIFY_NONE", "NULL"]),
        off
    );
    assert_eq!(
        verdict("SSL_set_verify", &["ssl", "SSL_VERIFY_PEER", "cb"]),
        Verdict::Suppress
    );
    assert_eq!(
        label("curl_easy_setopt", &["h", "CURLOPT_SSL_VERIFYPEER", "0L"]),
        off
    );
    assert_eq!(
        label("curl_easy_setopt", &["h", "CURLOPT_SSL_VERIFYHOST", "0"]),
        off
    );
    for zero in ["0l", "(long)0", "( long ) 0", "FALSE"] {
        let off_again = label("curl_easy_setopt", &["h", "CURLOPT_SSL_VERIFYPEER", zero]);
        assert_eq!(off_again, off, "{zero}");
    }
    assert_eq!(
        verdict("curl_easy_setopt", &["h", "CURLOPT_SSL_VERIFYPEER", "1L"]),
        Verdict::Suppress
    );
    assert_eq!(
        verdict("curl_easy_setopt", &["h", "CURLOPT_URL", "url"]),
        Verdict::Suppress
    );
}

#[test]
fn judges_every_name_in_its_tables() {
    let judged: std::collections::HashSet<&str> = judged_names().collect();
    for name in [
        "printk",
        "_stscanf",
        "lstrcpyA",
        "mempcpy",
        "_tsystem",
        "__builtin_alloca",
        "kvmalloc",
        "SSL_set_verify",
        "curl_easy_setopt",
        "realpath",
    ] {
        assert!(judged.contains(name), "{name}");
    }
    assert!(!judged.contains("free") && !judged.contains("copy_from_user"));
}
