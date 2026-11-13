use super::*;

fn build(text: &str) -> BuildId {
    text.parse().unwrap()
}

#[test]
fn build_id_parses_valve_ids() {
    let id = build("3.0.20261021.266025");
    assert_eq!(id.parts(), &[3, 0, 20261021, 266025]);
    assert_eq!(id.to_string(), "3.0.20261021.266025");
    // Old-style ids from before 2023 parse too.
    assert_eq!(build("0.20220119.0").parts(), &[0, 20220119, 0]);
    assert_eq!(build(" 1.0.1 \n").as_str(), "1.0.1");
}

#[test]
fn build_id_rejects_garbage() {
    for text in [
        "",
        "3..0",
        ".3",
        "3.",
        "3.0-beta",
        "v3.0",
        "3.0.x",
        "99999999999999999999999",
    ] {
        assert!(text.parse::<BuildId>().is_err(), "{text:?} parsed");
    }
}

#[test]
fn build_id_orders_numerically() {
    assert!(build("3.0.20261021.266025") < build("3.0.20261110.268881"));
    assert!(build("3.0.9") < build("3.0.10"));
    assert!(build("3.0") < build("3.0.0"));
    assert!(build("0.20231211.70175") < build("3.0.20240104.72532"));
    let mut ids = [build("2.0.1"), build("10.0.0"), build("2.0.0")];
    ids.sort();
    assert_eq!(
        ids.iter().map(BuildId::as_str).collect::<Vec<_>>(),
        ["2.0.0", "2.0.1", "10.0.0"]
    );
}

#[test]
fn build_id_order_is_consistent_with_eq() {
    let a = build("1.01");
    let b = build("1.1");
    assert_ne!(a, b);
    assert_ne!(a.cmp(&b), std::cmp::Ordering::Equal);
}

#[test]
fn runtime_ref_forms() {
    let plain: RuntimeRef = "sniper".parse().unwrap();
    assert_eq!(plain.name, "sniper");
    assert!(!plain.is_pinned());
    assert_eq!(plain.to_string(), "sniper");

    let pinned: RuntimeRef = "scout-ldlp=1.0.20261021.266054".parse().unwrap();
    assert_eq!(pinned.name, "scout-ldlp");
    assert_eq!(pinned.build, Some(build("1.0.20261021.266054")));
    assert_eq!(pinned.to_string(), "scout-ldlp=1.0.20261021.266054");
}

#[test]
fn runtime_ref_rejects_bad_names_and_builds() {
    for text in [
        "",
        "-S",
        "..",
        "snip er",
        "sniper/x",
        "sniper=",
        "sniper=abc",
        "=3.0",
    ] {
        assert!(text.parse::<RuntimeRef>().is_err(), "{text:?} parsed");
    }
}

#[test]
fn channel_parse_and_pointers() {
    assert_eq!("beta".parse::<Channel>().unwrap(), Channel::Beta);
    assert!("testing".parse::<Channel>().is_err());
    assert_eq!(Channel::default(), Channel::Stable);
    assert_eq!(Channel::Stable.pointer_files(), ["latest-public-stable"]);
    assert_eq!(
        Channel::Steam.pointer_files(),
        [
            "latest-container-runtime-depot",
            "latest-steam-client-general-availability"
        ]
    );
    for channel in Channel::ALL {
        assert_eq!(channel.as_str().parse::<Channel>().unwrap(), channel);
    }
}

#[test]
fn serde_uses_string_forms() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Doc {
        runtime: RuntimeRef,
        channel: Channel,
    }
    let doc: Doc = toml::from_str("runtime = \"sniper=3.0.1\"\nchannel = \"steam\"\n").unwrap();
    assert_eq!(doc.runtime.to_string(), "sniper=3.0.1");
    assert_eq!(doc.channel, Channel::Steam);
    assert_eq!(
        toml::to_string(&doc).unwrap(),
        "runtime = \"sniper=3.0.1\"\nchannel = \"steam\"\n"
    );

    let err = toml::from_str::<Doc>("runtime = \"sniper=x\"\nchannel = \"beta\"\n").unwrap_err();
    assert!(err.to_string().contains("is not a build id"), "{err}");
}
