use serde_json::{Value, json};

use super::{FingerprintError, canonical, fingerprint, jcs};

/// The fingerprint of the spec 12.2 example, computed outside this crate: the example's JSON without
/// `docs` and `name`, keys sorted, no whitespace, then SHA-256.
pub(crate) const SPEC_EXAMPLE: &str = "dedf47918edc5b0734cae064374cbb478f796a735cbd1ad7d2584f109a610097";

fn spec_example() -> Value {
  let spec = include_str!("../../../../spec/pavia-protocol.md");
  let section = &spec[spec.find("### 12.2 Manifest format").expect("section 12.2")..];
  let start = section.find("```json").expect("the example") + "```json".len();
  let block = &section[start..];
  serde_json::from_str(&block[..block.find("```").expect("the example's end")]).expect("the example parses")
}

pub(crate) fn hex(bytes: [u8; 32]) -> String {
  bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn fingerprints_the_example_of_spec_12_2() {
  assert_eq!(hex(fingerprint(&spec_example()).unwrap()), SPEC_EXAMPLE);
}

#[test]
fn writes_the_rfc_8785_example() {
  let input: Value = serde_json::from_str(
    r#"{"numbers": [333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
        "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
        "literals": [null, true, false]}"#,
  )
  .unwrap();
  assert_eq!(
    jcs(&input),
    r#"{"literals":[null,true,false],"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],"string":"€$\u000f\nA'B\"\\\\\"/"}"#
  );
}

#[test]
fn sorts_members_by_utf16_code_units() {
  let input: Value = serde_json::from_str(
    r#"{"€": "Euro Sign", "\r": "Carriage Return", "דּ": "Hebrew Letter Dalet With Dagesh",
        "1": "One", "😀": "Emoji: Grinning Face", "\u0080": "Control", "ö": "Latin Small Letter O With Diaeresis"}"#,
  )
  .unwrap();
  assert_eq!(
    jcs(&input),
    concat!(
      "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u{80}\":\"Control\",",
      "\"\u{f6}\":\"Latin Small Letter O With Diaeresis\",\"\u{20ac}\":\"Euro Sign\",",
      "\"\u{1f600}\":\"Emoji: Grinning Face\",\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
    )
  );
}

#[test]
fn writes_numbers_as_ecmascript_does() {
  for (number, text) in [
    (json!(0), "0"),
    (json!(-0.0), "0"),
    (json!(1), "1"),
    (json!(-1), "-1"),
    (json!(9_007_199_254_740_992_u64), "9007199254740992"),
    (json!(9_007_199_254_740_993_u64), "9007199254740992"),
    (json!(-9_007_199_254_740_993_i64), "-9007199254740992"),
    (json!(1e21), "1e+21"),
    (json!(1e20), "100000000000000000000"),
    (json!(0.000_001), "0.000001"),
    (json!(1e-7), "1e-7"),
    (json!(5e-324), "5e-324"),
    (json!(1.797_693_134_862_315_7e308), "1.7976931348623157e+308"),
  ] {
    assert_eq!(jcs(&number), text, "{number}");
  }
}

#[test]
fn leaves_out_docs_and_the_top_level_name() {
  let documented = json!({
    "pavia": 1,
    "name": "one-name",
    "docs": "The app.",
    "services": [{"name": "chat", "docs": "Chat.", "server_methods": [], "client_methods": []}],
    "types": {"Room": {"struct": {"fields": [{"name": "name", "type": "string", "docs": "The name."}]}}}
  });
  let plain = json!({
    "pavia": 1,
    "name": "another-name",
    "services": [{"name": "chat", "server_methods": [], "client_methods": []}],
    "types": {"Room": {"struct": {"fields": [{"name": "name", "type": "string"}]}}}
  });
  assert_eq!(canonical(&documented), canonical(&plain));
  assert_eq!(fingerprint(&documented), fingerprint(&plain));
  assert!(canonical(&plain).unwrap().contains(r#""name":"chat""#));
}

#[test]
fn keeps_a_type_named_docs_and_leaves_out_its_docs() {
  let named_docs = |definition: Value| json!({"pavia": 1, "services": [], "types": {"docs": definition}});
  assert_eq!(
    canonical(&named_docs(json!({"alias": "string", "docs": "A name."}))).unwrap(),
    r#"{"pavia":1,"services":[],"types":{"docs":{"alias":"string"}}}"#
  );
  assert_ne!(
    fingerprint(&named_docs(json!({"alias": "string"}))),
    fingerprint(&named_docs(json!({"alias": "u32"})))
  );
}

#[test]
fn refuses_null_and_default_values() {
  let mut example = spec_example();
  example["services"][0]["server_methods"][1]["idempotent"] = json!(false);
  assert_eq!(
    fingerprint(&example),
    Err(FingerprintError::Default {
      at: "$.services[0].server_methods[1].idempotent".into()
    })
  );
  let mut example = spec_example();
  example["services"][0]["server_methods"][3]["params"] = json!([]);
  assert_eq!(
    fingerprint(&example),
    Err(FingerprintError::Default {
      at: "$.services[0].server_methods[3].params".into()
    })
  );
  for (key, value) in [("returns", json!("unit")), ("returns", Value::Null), ("docs", Value::Null)] {
    let mut example = spec_example();
    example["services"][0]["client_methods"][0][key] = value;
    assert!(fingerprint(&example).is_err(), "{key}");
  }
  let mut example = spec_example();
  example["channels"][0]["history"] = json!(false);
  assert!(matches!(fingerprint(&example), Err(FingerprintError::Default { .. })));
  let mut example = spec_example();
  example["types"]["Member"]["struct"]["fields"][0]["optional"] = json!(false);
  assert!(fingerprint(&example).is_ok(), "12.2 lists no default for optional");
  let mut example = spec_example();
  example["types"]["SendError"]["enum"]["open"] = json!(false);
  assert!(matches!(fingerprint(&example), Err(FingerprintError::Default { .. })));
  assert_eq!(
    fingerprint(&json!({"pavia": 1, "services": [null]})),
    Err(FingerprintError::Null {
      at: "$.services[0]".into()
    })
  );
  assert_eq!(fingerprint(&json!([])), Err(FingerprintError::NotAnObject { at: "$".into() }));
}

#[test]
fn refuses_named_entries_out_of_byte_order_and_keeps_params_in_theirs() {
  let mut example = spec_example();
  let methods = example["services"][0]["server_methods"].as_array_mut().unwrap();
  methods.swap(0, 1);
  assert_eq!(
    fingerprint(&example),
    Err(FingerprintError::Unsorted {
      at: "$.services[0].server_methods".into(),
      key: "name"
    })
  );
  let mut swapped = spec_example();
  let params = swapped["services"][0]["server_methods"][1]["params"].as_array_mut().unwrap();
  params.swap(0, 1);
  assert_ne!(fingerprint(&swapped).unwrap(), fingerprint(&spec_example()).unwrap());
  let unsorted = [
    json!({"services": [{"name": "b"}, {"name": "a"}]}),
    json!({"services": [{"name": "a"}, {"name": "a"}]}),
    json!({"services": [{"name": "B"}, {"name": "a"}, {"name": "A"}]}),
    json!({"channels": [{"namespace": "room"}, {"namespace": "feed"}]}),
    json!({"datagrams": [{"topic": "pos"}, {"topic": "cursor"}]}),
    json!({"services": [{"name": "a", "client_methods": [{"name": "y"}, {"name": "x"}]}]}),
    json!({"services": [{"title": "a"}]}),
  ];
  for manifest in unsorted {
    assert!(
      matches!(fingerprint(&manifest), Err(FingerprintError::Unsorted { .. })),
      "{manifest}"
    );
  }
  assert!(fingerprint(&json!({"services": [{"name": "B"}, {"name": "a"}]})).is_ok());
}

#[test]
fn writes_control_characters_with_their_short_escapes() {
  assert_eq!(jcs(&json!("\u{8}\u{c}\t\n\r\u{1}")), r#""\b\f\t\n\r\u0001""#);
}

#[test]
fn fingerprints_a_manifest_without_named_arrays() {
  assert_eq!(canonical(&json!({"pavia": 1, "name": "app"})).unwrap(), r#"{"pavia":1}"#);
}

#[test]
fn refuses_named_lists_that_are_not_arrays() {
  for (manifest, at) in [
    (json!({"services": {}}), "$.services"),
    (json!({"channels": "room"}), "$.channels"),
    (json!({"datagrams": 1}), "$.datagrams"),
    (
      json!({"services": [{"name": "a", "server_methods": {}}]}),
      "$.services[0].server_methods",
    ),
  ] {
    assert_eq!(
      fingerprint(&manifest),
      Err(FingerprintError::NotAnArray { at: at.into() }),
      "{manifest}"
    );
  }
}

#[test]
fn strips_docs_inside_every_top_level_member_but_the_type_names() {
  let manifest = json!({"pavia": 1, "extra": {"docs": "Gone.", "kept": 1}, "types": {"docs": {"alias": "string"}}});
  assert_eq!(
    canonical(&manifest).unwrap(),
    r#"{"extra":{"kept":1},"pavia":1,"types":{"docs":{"alias":"string"}}}"#
  );
}
