use super::*;

const SETTERS: [fn(&CookieJar, &str, &Url); 2] = [
    CookieJar::set_cookie,
    CookieJar::set_cookie_from_js,
];

#[test]
fn explicit_exact_dns_domain_reaches_child_but_absent_and_empty_do_not() {
    let origin = Url::parse("http://example.test/account/page").unwrap();
    let child = Url::parse("http://one.example.test/account/page").unwrap();
    for setter in SETTERS {
        for (attribute, child_header) in [
            ("", ""),
            ("; Domain=", ""),
            ("; Domain=example.test", "id=value"),
            ("; Domain=.EXAMPLE.test", "id=value"),
        ] {
            let jar = CookieJar::new();
            setter(&jar, &format!("id=value; Path=/account{attribute}"), &origin);
            assert_eq!(jar.get_cookie_header(&origin), "id=value", "{attribute}");
            assert_eq!(jar.get_cookie_header(&child), child_header, "{attribute}");
            assert_eq!(jar.get_js_visible_cookies(&child), child_header, "{attribute}");
            let entry = jar.snapshot().entries.into_values().next().unwrap();
            assert_eq!(entry.host_only, child_header.is_empty(), "{attribute}");
        }
    }
}

#[test]
fn same_name_path_host_and_domain_cookies_coexist_and_delete_selectively() {
    let origin = Url::parse("https://example.test/account/page").unwrap();
    let child = Url::parse("https://one.example.test/account/page").unwrap();
    for setter in SETTERS {
        for reversed in [false, true] {
            let jar = CookieJar::new();
            let host = "id=H; Path=/account";
            let domain = "id=D; Domain=example.test; Path=/account";
            for assignment in if reversed { [domain, host] } else { [host, domain] } {
                setter(&jar, assignment, &origin);
            }
            let header = if reversed { "id=D; id=H" } else { "id=H; id=D" };
            assert_eq!(jar.get_cookie_header(&origin), header);
            assert_eq!(jar.get_js_visible_cookies(&origin), header);
            assert_eq!(jar.snapshot().entries.len(), 2);
            assert_eq!(jar.get_cookie_header(&child), "id=D");

            setter(&jar, "id=; Path=/account; Max-Age=0", &origin);
            assert_eq!(jar.get_cookie_header(&origin), "id=D");
            assert_eq!(jar.get_cookie_header(&child), "id=D");
            setter(&jar, "id=H2; Path=/account", &origin);
            assert_eq!(jar.get_cookie_header(&origin), "id=D; id=H2");
            setter(&jar, "id=; Domain=.example.test; Path=/account; Max-Age=0", &origin);
            assert_eq!(jar.get_cookie_header(&origin), "id=H2");
            assert!(jar.get_cookie_header(&child).is_empty());
        }
    }
}

#[test]
fn domain_classifier_respects_private_wildcard_exception_and_unknown_suffixes() {
    for setter in SETTERS {
        for (host, attribute, expected) in [
            ("www.example.co.uk", "example.co.uk", Some(("example.co.uk", false))),
            ("www.example.co.uk", "co.uk", None),
            ("sub.gov.uk", "gov.uk", None),
            ("gov.uk", ".GOV.uk", Some(("gov.uk", true))),
            ("foo.github.io", "github.io", None),
            ("github.io", "github.io", Some(("github.io", true))),
            ("foo.github.io", ".foo.github.io", Some(("foo.github.io", false))),
            ("one.foo.ck", "foo.ck", None),
            ("foo.ck", "foo.ck", Some(("foo.ck", true))),
            ("www.ck", "www.ck", Some(("www.ck", false))),
            ("one.www.ck", "www.ck", Some(("www.ck", false))),
            ("example.invalid", "example.invalid", Some(("example.invalid", false))),
            ("one.example.invalid", "invalid", None),
            ("intranet", "intranet", Some(("intranet", true))),
            ("one.intranet", "intranet", None),
        ] {
            let origin = Url::parse(&format!("https://{host}/")).unwrap();
            let jar = CookieJar::new();
            setter(&jar, &format!("id=value; Domain={attribute}; Path=/"), &origin);
            let snapshot = jar.snapshot();
            if let Some((domain, host_only)) = expected {
                assert_eq!(snapshot.entries.len(), 1, "{host}, {attribute}");
                let entry = snapshot.entries.into_values().next().unwrap();
                assert_eq!((entry.domain.as_str(), entry.host_only), (domain, host_only));
                assert_eq!(jar.get_cookie_header(&origin), "id=value");
            } else {
                assert!(snapshot.entries.is_empty(), "{host}, {attribute}");
                assert_eq!(jar.next_creation_order.load(Ordering::Relaxed), 1);
            }
        }
    }
}

#[test]
fn exact_ip_and_local_domains_stay_host_only_without_ip_normalization() {
    for setter in SETTERS {
        for host in ["localhost", "intranet", "127.0.0.1", "[::1]"] {
            let origin = Url::parse(&format!("http://{host}/")).unwrap();
            for attribute in [String::new(), "; Domain=".to_string(),
                format!("; Domain={host}"), format!("; Domain=.{host}")]
            {
                let jar = CookieJar::new();
                setter(&jar, &format!("id=value; Path=/{attribute}"), &origin);
                assert_eq!(jar.get_cookie_header(&origin), "id=value");
                assert!(jar.snapshot().entries.values().all(|entry| entry.host_only));
            }
        }
        for (host, invalid) in [
            ("127.0.0.1", "127.1"), ("127.0.0.1", "2130706433"),
            ("127.0.0.1", "0.0.1"), ("127.0.0.1", "127.0.0.2"),
            ("[::1]", "[0:0:0:0:0:0:0:1]"), ("[::1]", "::1"),
            ("localhost", "local"), ("intranet", "other"),
        ] {
            let jar = CookieJar::new();
            let origin = Url::parse(&format!("http://{host}/")).unwrap();
            setter(&jar, &format!("id=value; Domain={invalid}"), &origin);
            assert!(jar.snapshot().entries.is_empty(), "{host}, {invalid}");
        }
    }
}

#[test]
fn invalid_assignments_and_expiry_do_not_mutate_cookie_or_order() {
    let origin = Url::parse("https://example.test/account").unwrap();
    for setter in SETTERS {
        for attribute in ["victim.test", "test", ".", "..example.test", "example..test",
            "example.test..", "%65xample.test", "example.test:443", "example/test",
            "example.test.", "[::1]", " example. test", "\"example.test\""]
        {
            let jar = CookieJar::new();
            jar.set_cookie("id=H; Path=/", &origin);
            jar.set_cookie("id=D; Domain=example.test; Path=/", &origin);
            let initial = jar.snapshot();
            let order = jar.next_creation_order.load(Ordering::Relaxed);
            for expiry in ["", "; Max-Age=0"] {
                setter(&jar, &format!("id=changed; Domain={attribute}; Path=/{expiry}"), &origin);
                assert_eq!(jar.snapshot().entries, initial.entries, "{attribute}, {expiry}");
                assert_eq!(jar.next_creation_order.load(Ordering::Relaxed), order);
            }
        }
        let jar = CookieJar::new();
        setter(&jar, "id=value; Domain=other.test; Domain=example.test", &origin);
        assert_eq!(jar.get_cookie_header(&origin), "id=value");
        setter(&jar, "id=ignored; Domain=example.test; Domain=other.test", &origin);
        assert_eq!(jar.get_cookie_header(&origin), "id=value");
    }
}

#[test]
fn dns_trailing_dot_is_preserved_and_not_interchangeable() {
    for setter in SETTERS {
        for (host, expected_host_only) in [
            ("example.test.", false), ("gov.uk.", true), ("localhost.", true),
        ] {
            let origin = Url::parse(&format!("https://{host}/")).unwrap();
            let child = Url::parse(&format!("https://one.{host}/")).unwrap();
            let jar = CookieJar::new();
            setter(&jar, &format!("id=value; Domain=.{host}; Path=/"), &origin);
            let entry = jar.snapshot().entries.into_values().next().unwrap();
            assert_eq!(entry.domain, host);
            assert_eq!(entry.host_only, expected_host_only);
            assert_eq!(jar.get_cookie_header(&child), if expected_host_only { "" } else { "id=value" });
            let before = jar.snapshot();
            setter(&jar, &format!("id=; Domain={}; Path=/; Max-Age=0", host.trim_end_matches('.')), &origin);
            assert_eq!(jar.snapshot().entries, before.entries);
        }
        let invalid_origin = Url::parse("https://example.test../").unwrap();
        let jar = CookieJar::new();
        setter(&jar, "id=value", &invalid_origin);
        assert!(jar.snapshot().entries.is_empty());
    }
}

#[test]
fn ascii_punycode_domain_works_without_expanding_raw_unicode_acceptance() {
    let origin = Url::parse("https://bücher.example/").unwrap();
    let child = Url::parse("https://one.xn--bcher-kva.example/").unwrap();
    for setter in SETTERS {
        let jar = CookieJar::new();
        setter(&jar, "host=H; Path=/", &origin);
        setter(&jar, "domain=D; Domain=xn--bcher-kva.example; Path=/", &origin);
        assert_eq!(jar.get_cookie_header(&origin), "host=H; domain=D");
        assert_eq!(jar.get_cookie_header(&child), "domain=D");
        let before = jar.snapshot();
        // Raw Unicode Domain was not accepted by the previous resolver. This
        // retained boundary is not Chromium's feature-dependent IDNA parity.
        setter(&jar, "unicode=unsupported; Domain=bücher.example; Path=/", &origin);
        assert_eq!(jar.snapshot().entries, before.entries);
    }
}

#[test]
fn httponly_guards_target_scope_without_exposing_or_mutating_sibling() {
    let origin = Url::parse("https://example.test/").unwrap();
    for protected_host in [true, false] {
        let protected_attr = if protected_host { "" } else { "; Domain=example.test" };
        let visible_attr = if protected_host { "; Domain=example.test" } else { "" };
        let jar = CookieJar::new();
        jar.set_cookie(&format!("id=secret; Path=/; HttpOnly{protected_attr}"), &origin);
        let original = jar.snapshot();
        let order = jar.next_creation_order.load(Ordering::Relaxed);
        for assignment in [format!("id=overwrite; Path=/{protected_attr}"),
            format!("id=; Path=/; Max-Age=0{protected_attr}")]
        {
            jar.set_cookie_from_js(&assignment, &origin);
            assert_eq!(jar.snapshot().entries, original.entries);
            assert_eq!(jar.next_creation_order.load(Ordering::Relaxed), order);
        }
        jar.set_cookie_from_js(&format!("id=visible; Path=/{visible_attr}"), &origin);
        assert_eq!(jar.get_cookie_header(&origin), "id=secret; id=visible");
        assert_eq!(jar.get_js_visible_cookies(&origin), "id=visible");
        jar.set_cookie_from_js(&format!("id=; Path=/; Max-Age=0{visible_attr}"), &origin);
        assert_eq!(jar.snapshot().entries, original.entries);
        assert_eq!(jar.get_cookie_header(&origin), "id=secret");
        assert!(jar.get_js_visible_cookies(&origin).is_empty());
    }
}

#[test]
fn scope_siblings_preserve_path_secure_and_value_sensitive_order() {
    let origin = Url::parse("https://example.test/account/page").unwrap();
    let insecure = Url::parse("http://example.test/account/page").unwrap();
    for setter in SETTERS {
        let jar = CookieJar::new();
        setter(&jar, "id=H; Path=/account; Secure", &origin);
        setter(&jar, "id=D; Domain=example.test; Path=/account", &origin);
        setter(&jar, "id=root; Path=/", &origin);
        assert_eq!(jar.get_cookie_header(&origin), "id=H; id=D; id=root");
        assert_eq!(jar.get_cookie_header(&insecure), "id=D; id=root");
        let order = jar.next_creation_order.load(Ordering::Relaxed);
        setter(&jar, "id=D; Domain=.EXAMPLE.test; Path=/account; Max-Age=3600", &origin);
        assert_eq!(jar.get_cookie_header(&origin), "id=H; id=D; id=root");
        assert_eq!(jar.next_creation_order.load(Ordering::Relaxed), order);
        setter(&jar, "id=D2; Domain=.EXAMPLE.test; Path=/account", &origin);
        assert_eq!(jar.get_cookie_header(&origin), "id=H; id=D2; id=root");
        setter(&jar, "id=H2; Path=/account; Secure", &origin);
        assert_eq!(jar.get_cookie_header(&origin), "id=D2; id=H2; id=root");
        assert_eq!(jar.get_cookie_header(&Url::parse("https://example.test/other").unwrap()), "id=root");
    }
}

#[test]
fn paired_snapshot_delta_preserves_concurrent_sibling_and_scope_specific_deletion() {
    let origin = Url::parse("https://example.test/").unwrap();
    let destination = CookieJar::new();
    destination.set_cookie("id=H; Path=/", &origin);
    destination.set_cookie("id=D; Domain=example.test; Path=/", &origin);
    let initial = destination.snapshot();
    let connection = CookieJar::from_snapshot(&initial);
    assert_eq!(connection.snapshot().entries, initial.entries);
    destination.set_cookie("id=concurrent; Domain=example.test; Path=/", &origin);
    connection.set_cookie_from_js("id=; Path=/; Max-Age=0", &origin);
    connection.set_cookie("first=one; Path=/", &origin);
    connection.set_cookie("id=H2; Path=/", &origin);
    destination.apply_snapshot_delta(&initial, &connection.snapshot());
    assert_eq!(destination.get_cookie_header(&origin), "id=concurrent; first=one; id=H2");
    let child = Url::parse("https://one.example.test/").unwrap();
    assert_eq!(destination.get_cookie_header(&child), "id=concurrent");
    assert_eq!(connection.get_cookie_header(&origin), "id=D; first=one; id=H2");
}

#[test]
fn copied_expired_scope_is_not_a_delete_of_concurrent_refresh() {
    let origin = Url::parse("https://example.test/").unwrap();
    for expired_host in [true, false] {
        let source = CookieJar::new();
        source.set_cookie("id=H; Path=/", &origin);
        source.set_cookie("id=D; Domain=example.test; Path=/", &origin);
        source.cookies.write().unwrap().get_mut("example.test").unwrap()
            .get_mut(&cookie_storage_key("id", "/", expired_host)).unwrap().expires = Some(0);
        let initial = source.snapshot();
        let connection = CookieJar::from_snapshot(&initial);
        let destination = CookieJar::from_snapshot(&initial);
        let attribute = if expired_host { "" } else { "; Domain=example.test" };
        destination.set_cookie(&format!("id=refreshed; Path=/{attribute}"), &origin);
        let refreshed = destination.snapshot();
        destination.apply_snapshot_delta(&initial, &connection.snapshot());
        assert_eq!(destination.snapshot().entries, refreshed.entries,
            "copying natural expiry must not delete a concurrent refresh, host_only={expired_host}");
        assert_eq!(connection.snapshot().entries, initial.entries);
        assert_eq!(connection.get_cookie_header(&origin), if expired_host { "id=D" } else { "id=H" });
        assert_eq!(connection.get_all_cookies().len(), 1);
        connection.set_cookie(&format!("id=recreated; Path=/{attribute}"), &origin);
        let key = ("example.test".to_string(), cookie_storage_key("id", "/", expired_host));
        assert!(connection.snapshot().entries[&key].creation_order >
            initial.entries.values().map(|entry| entry.creation_order).max().unwrap());
    }
}

#[test]
fn expired_httponly_copy_allows_js_recreation_but_protects_live_sibling() {
    let origin = Url::parse("https://example.test/").unwrap();
    for expired_host in [true, false] {
        let source = CookieJar::new();
        source.set_cookie("id=H; Path=/; HttpOnly", &origin);
        source.set_cookie("id=D; Domain=example.test; Path=/; HttpOnly", &origin);
        source.cookies.write().unwrap().get_mut("example.test").unwrap()
            .get_mut(&cookie_storage_key("id", "/", expired_host)).unwrap().expires = Some(0);
        let initial = source.snapshot();
        let expired_attribute = if expired_host { "" } else { "; Domain=example.test" };
        let live_attribute = if expired_host { "; Domain=example.test" } else { "" };
        let live_value = if expired_host { "D" } else { "H" };
        let copied = CookieJar::from_snapshot(&initial);
        copied.set_cookie_from_js(&format!("id=recreated; Path=/{expired_attribute}"), &origin);
        assert_eq!(copied.get_cookie_header(&origin), format!("id={live_value}; id=recreated"));
        assert_eq!(copied.get_js_visible_cookies(&origin), "id=recreated");
        let recreated = copied.snapshot();
        for assignment in [format!("id=blocked; Path=/{live_attribute}"),
            format!("id=; Max-Age=0; Path=/{live_attribute}")]
        {
            copied.set_cookie_from_js(&assignment, &origin);
            assert_eq!(copied.snapshot().entries, recreated.entries);
        }
        let deleted = CookieJar::from_snapshot(&initial);
        let order = deleted.next_creation_order.load(Ordering::Relaxed);
        deleted.set_cookie_from_js(&format!("id=; Max-Age=0; Path=/{expired_attribute}"), &origin);
        assert_eq!(deleted.snapshot().entries.len(), 1);
        assert_eq!(deleted.next_creation_order.load(Ordering::Relaxed), order);
        let destination = CookieJar::from_snapshot(&initial);
        destination.set_cookie(&format!("id=refresh; Path=/; HttpOnly{expired_attribute}"), &origin);
        destination.apply_snapshot_delta(&initial, &deleted.snapshot());
        assert_eq!(destination.get_cookie_header(&origin), format!("id={live_value}"));
    }
}

#[test]
fn paired_version1_roundtrip_and_expired_merge_preserve_sibling_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cookies.json");
    let origin = Url::parse("https://example.test/").unwrap();
    let source = CookieJar::new();
    source.set_cookie("id=H; Path=/; HttpOnly", &origin);
    source.set_cookie("id=D; Domain=example.test; Path=/; Secure", &origin);
    source.save_to_file(&path).unwrap();
    let mut saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], 1);
    saved["cookies"].as_array_mut().unwrap().reverse();
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let restored = CookieJar::new();
    assert_eq!(restored.load_from_file(&path).unwrap(), 2);
    assert_eq!(restored.snapshot().entries, source.snapshot().entries);
    assert_eq!(restored.get_js_visible_cookies(&origin), "id=D");
    assert_eq!(restored.get_cookie_header(&Url::parse("https://one.example.test/").unwrap()), "id=D");

    for entry in saved["cookies"].as_array_mut().unwrap() {
        if entry["host_only"] == true { entry["expires"] = serde_json::json!(0); }
    }
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    restored.load_from_file(&path).unwrap();
    assert_eq!(restored.get_cookie_header(&origin), "id=D");
    assert_eq!(restored.snapshot().entries.len(), 1);
}

#[test]
fn lossy_cdp_projection_reimport_is_domain_only_and_bulk_delete_covers_both_scopes() {
    let origin = Url::parse("https://example.test/").unwrap();
    let jar = CookieJar::new();
    jar.set_cookie("id=H; Path=/", &origin);
    jar.set_cookie("id=D; Domain=example.test; Path=/", &origin);
    let projected = jar.get_all_cookies();
    assert_eq!(projected.len(), 2);
    assert_eq!(projected[0].domain, projected[1].domain);
    let empty = CookieJar::new();
    empty.set_cookies_from_cdp(projected.clone());
    assert_eq!(empty.get_cookie_header(&origin), "id=D");
    assert_eq!(empty.snapshot().entries.len(), 1);
    assert!(!empty.snapshot().entries.values().next().unwrap().host_only);
    let initial = jar.snapshot();
    jar.set_cookies_from_cdp(projected);
    assert_eq!(jar.snapshot().entries, initial.entries);

    let mut expired = jar.get_all_cookies().into_iter().find(|entry| entry.value == "D").unwrap();
    expired.domain = ".EXAMPLE.test".to_string();
    expired.expires = Some(0);
    jar.set_cookies_from_cdp(vec![expired]);
    assert_eq!(jar.get_cookie_header(&origin), "id=H");
    jar.set_cookie("id=D2; Domain=example.test; Path=/", &origin);
    jar.delete_cookies_filtered("id", ".example.test", Some("/other"));
    assert_eq!(jar.get_cookie_header(&origin), "id=H; id=D2");
    jar.delete_cookies_filtered("id", ".example.test", Some("/"));
    assert!(jar.get_all_cookies().is_empty());
    jar.set_cookie("id=H; Path=/", &origin);
    jar.set_cookie("id=D; Domain=example.test; Path=/", &origin);
    jar.delete_cookie("id", "example.test");
    assert!(jar.snapshot().entries.is_empty());
}
