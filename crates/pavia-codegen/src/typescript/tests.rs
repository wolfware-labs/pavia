use std::collections::BTreeMap;

use pavia_contract::manifest::{Call, Manifest, Method, Param, Service};
use pavia_contract::schema::{Definition, Enum, Field, Primitive, Struct, Type, Variant};

use super::generate;
use crate::GenerateError;

fn prim(primitive: Primitive) -> Type {
  Type::from(primitive)
}

fn reference(name: &str) -> Type {
  Type::Ref(name.into())
}

fn with_types(types: Vec<(&str, Definition)>) -> Manifest {
  Manifest::new("app").types(
    types
      .into_iter()
      .map(|(name, definition)| (name.to_owned(), definition))
      .collect(),
  )
}

fn code(manifest: &Manifest) -> String {
  generate(manifest).unwrap_or_else(|error| panic!("{error}"))
}

fn has(code: &str, line: &str) {
  assert!(
    code.lines().any(|candidate| candidate == line),
    "no line `{line}` in:\n{code}"
  );
}

#[test]
fn maps_every_primitive() {
  let expected = [
    (Primitive::Unit, "null"),
    (Primitive::Bool, "boolean"),
    (Primitive::I8, "number"),
    (Primitive::I16, "number"),
    (Primitive::I32, "number"),
    (Primitive::I64, "bigint"),
    (Primitive::U8, "number"),
    (Primitive::U16, "number"),
    (Primitive::U32, "number"),
    (Primitive::U64, "bigint"),
    (Primitive::F32, "number"),
    (Primitive::F64, "number"),
    (Primitive::String, "string"),
    (Primitive::Bytes, "Uint8Array"),
    (Primitive::Timestamp, "Date"),
    (Primitive::Uuid, "string"),
    (Primitive::Any, "unknown"),
  ];
  assert_eq!(expected.len(), Primitive::ALL.len());
  for (primitive, ts) in expected {
    let code = code(&with_types(vec![("T", Definition::Alias(prim(primitive)))]));
    has(&code, &format!("export type T = {ts};"));
  }
}

#[test]
fn maps_every_composite_type() {
  let cases = [
    (Type::Option(Box::new(prim(Primitive::U8))), "number | null"),
    (Type::List(Box::new(prim(Primitive::String))), "string[]"),
    (Type::Map(Box::new(prim(Primitive::U64))), "Map<string, bigint>"),
    (
      Type::Tuple(vec![prim(Primitive::Bool), prim(Primitive::Bytes)]),
      "[boolean, Uint8Array]",
    ),
    (Type::Tuple(Vec::new()), "[]"),
    (
      Type::List(Box::new(Type::Option(Box::new(prim(Primitive::U8))))),
      "(number | null)[]",
    ),
    (
      Type::Struct(Struct::new(vec![
        Field::new("x", prim(Primitive::F32)),
        Field::new("label", Type::Option(Box::new(prim(Primitive::String)))).optional(),
      ])),
      "{ x: number; label?: string | null }",
    ),
    (Type::Struct(Struct::new(Vec::new())), "Record<string, never>"),
    (reference("Other"), "Other"),
  ];
  for (ty, ts) in cases {
    let code = code(&with_types(vec![
      ("T", Definition::Alias(ty)),
      ("Other", Definition::Alias(prim(Primitive::U8))),
    ]));
    has(&code, &format!("export type T = {ts};"));
  }
}

#[test]
fn writes_a_struct_without_fields_as_an_object_without_keys() {
  let code = code(&with_types(vec![("Empty", Definition::Struct(Struct::new(Vec::new())))]));
  has(&code, "export type Empty = Record<string, never>;");
}

#[test]
fn writes_a_named_struct_as_an_interface_with_optional_fields() {
  let code = code(&with_types(vec![(
    "Message",
    Definition::Struct(Struct::new(vec![
      Field::new("text", prim(Primitive::String)),
      Field::new("sent_at", prim(Primitive::Timestamp)),
      Field::new("lang", Type::Option(Box::new(prim(Primitive::String)))).optional(),
    ])),
  )]));
  let start = code.find("export interface Message {").expect("the interface");
  let body: Vec<&str> = code[start..].lines().take(5).collect();
  assert_eq!(
    body,
    [
      "export interface Message {",
      "  text: string;",
      "  sent_at: Date;",
      "  lang?: string | null;",
      "}"
    ]
  );
}

#[test]
fn writes_an_enum_as_a_tagged_union() {
  let code = code(&with_types(vec![(
    "SendError",
    Definition::Enum(Enum::new(vec![
      Variant::unit("RoomNotFound"),
      Variant::new(
        "TooLong",
        Type::Struct(Struct::new(vec![Field::new("max", prim(Primitive::U32))])),
      ),
    ])),
  )]));
  has(
    &code,
    "export type SendError = { kind: \"RoomNotFound\" } | { kind: \"TooLong\"; value: { max: number } };",
  );
  assert!(!code.contains("SendErrorInput"));
}

#[test]
fn gives_an_open_enum_a_catch_all_when_decoded_and_none_as_an_argument() {
  let code = code(&with_types(vec![
    (
      "Status",
      Definition::Enum(Enum::new(vec![Variant::unit("Idle"), Variant::unit("Busy")]).open()),
    ),
    (
      "Member",
      Definition::Struct(Struct::new(vec![Field::new("status", reference("Status"))])),
    ),
    ("Statuses", Definition::Alias(Type::List(Box::new(reference("Member"))))),
    ("Plain", Definition::Alias(prim(Primitive::U8))),
  ]));
  has(
    &code,
    "export type Status = { kind: \"Idle\" } | { kind: \"Busy\" } | { kind: \"Unknown\"; name: string };",
  );
  has(&code, "export type StatusInput = { kind: \"Idle\" } | { kind: \"Busy\" };");
  has(&code, "export interface MemberInput {");
  has(&code, "  status: StatusInput;");
  has(&code, "export type Statuses = Member[];");
  has(&code, "export type StatusesInput = MemberInput[];");
  assert!(!code.contains("PlainInput"));
}

fn chat() -> Manifest {
  let room = || Param::new("room", reference("RoomId"));
  let service = Service::new("chat")
    .server_method(
      Method::call(
        "send",
        Call::unary(reference("MessageId")).error(reference("SendError")).idempotent(),
      )
      .param(room())
      .param(Param::new("text", prim(Primitive::String))),
    )
    .server_method(Method::notify("typing").param(room()))
    .server_method(Method::call("history", Call::server_stream(reference("Message"))).param(room()))
    .server_method(Method::call(
      "upload",
      Call::client_stream(prim(Primitive::Bytes), prim(Primitive::U64)),
    ))
    .server_method(Method::call(
      "relay",
      Call::bidi(prim(Primitive::String), prim(Primitive::String)),
    ))
    .server_method(Method::call("clear", Call::unary(prim(Primitive::Unit))))
    .client_method(
      Method::call("confirm", Call::unary(prim(Primitive::Bool))).param(Param::new("prompt", prim(Primitive::String))),
    )
    .client_method(Method::notify("system_notice").param(Param::new("text", prim(Primitive::String))));
  Manifest::new("app").service(service).types(BTreeMap::from([
    ("RoomId".into(), Definition::Alias(prim(Primitive::String))),
    ("MessageId".into(), Definition::Alias(prim(Primitive::Uuid))),
    (
      "Message".into(),
      Definition::Struct(Struct::new(vec![Field::new("text", prim(Primitive::String))])),
    ),
    (
      "SendError".into(),
      Definition::Enum(Enum::new(vec![Variant::unit("RoomNotFound")])),
    ),
  ]))
}

#[test]
fn writes_a_client_class_per_service_with_a_method_per_server_method() {
  let code = code(&chat());
  for line in [
    "export class ChatClient {",
    "  readonly #connection: Connection;",
    "  async send(room: RoomId, text: string, options?: CallOptions): Promise<MessageId> {",
    "      return await this.#connection.call<MessageId>(CALLS[\"chat/send\"], [room, text], options);",
    "  typing(room: RoomId, options?: NotifyOptions): void {",
    "    this.#connection.notify(NOTIFICATIONS[\"chat/typing\"], [room], options);",
    "  async clear(options?: CallOptions): Promise<void> {",
    "    await this.#connection.call(CALLS[\"chat/clear\"], [], options);",
    "  history(room: RoomId, options?: CallOptions): AsyncIterable<Message> {",
    "    throw new Error(\"chat/history: streaming is not implemented until M5\");",
    "  upload(items: AsyncIterable<Uint8Array>, options?: CallOptions): Promise<bigint> {",
    "    return Promise.reject(new Error(\"chat/upload: streaming is not implemented until M5\"));",
    "  relay(items: AsyncIterable<string>, options?: CallOptions): AsyncIterable<string> {",
  ] {
    has(&code, line);
  }
}

#[test]
fn rethrows_a_typed_error_for_a_method_with_an_error_type() {
  let code = code(&chat());
  for line in [
    "export class ChatSendError extends StatusError {",
    "  override readonly name = \"ChatSendError\";",
    "  declare readonly detail: SendError | undefined;",
    "      if (error instanceof StatusError) throw ChatSendError.from(error);",
  ] {
    has(&code, line);
  }
  assert!(!code.contains("ChatClearError"));
}

#[test]
fn writes_signatures_with_the_contract_types_and_the_idempotent_flag() {
  let code = code(&chat());
  let calls = code.find("const CALLS = {").expect("the call signatures");
  let send = &code[calls..];
  assert!(send.contains(
    "  \"chat/send\": {\n    method: \"chat/send\",\n    params: [{\"ref\":\"RoomId\"},\"string\"],\n    returns: \
     {\"ref\":\"MessageId\"},\n    error: {\"ref\":\"SendError\"},\n    idempotent: true,\n    types: TYPES,\n  },"
  ));
  assert!(code.contains("} satisfies Record<string, CallSignature>;"));
  assert!(code.contains("} satisfies Record<string, NotifySignature>;"));
}

#[test]
fn writes_the_signatures_of_a_module_with_only_calls_or_only_notifications() {
  let calls = code(&Manifest::new("app").service(
    Service::new("s").server_method(Method::call("get", Call::unary(prim(Primitive::U8)))),
  ));
  assert!(calls.contains("const TYPES: Types = {};"), "{calls}");
  assert!(calls.contains("const CALLS = {"), "{calls}");
  assert!(!calls.contains("const NOTIFICATIONS"), "{calls}");
  let notifications = code(&Manifest::new("app").service(Service::new("s").server_method(Method::notify("n"))));
  assert!(notifications.contains("const TYPES: Types = {};"), "{notifications}");
  assert!(notifications.contains("const NOTIFICATIONS = {"), "{notifications}");
  assert!(!notifications.contains("const CALLS"), "{notifications}");
}

#[test]
fn gives_an_error_class_only_to_a_unary_call_with_an_error_type() {
  let manifest = Manifest::new("app")
    .service(
      Service::new("chat")
        .server_method(Method::notify("typing"))
        .server_method(Method::call(
          "upload",
          Call::client_stream(prim(Primitive::Bytes), prim(Primitive::U64)).error(reference("Failure")),
        )),
    )
    .types(BTreeMap::from([
      ("Failure".into(), Definition::Alias(prim(Primitive::String))),
      ("ChatTypingError".into(), Definition::Alias(prim(Primitive::U8))),
      ("ChatUploadError".into(), Definition::Alias(prim(Primitive::U8))),
    ]));
  let code = code(&manifest);
  assert!(!code.contains("class ChatUploadError"), "{code}");
}

#[test]
fn types_the_client_methods_as_handlers() {
  let code = code(&chat());
  has(&code, "export interface ChatHandlers {");
  has(&code, "  confirm(prompt: string): boolean | Promise<boolean>;");
  has(&code, "  system_notice(text: string): void | Promise<void>;");
}

#[test]
fn embeds_the_fingerprint_of_the_manifest() {
  let manifest = chat();
  let bytes = manifest.fingerprint().unwrap();
  let rows: Vec<String> = bytes
    .chunks(16)
    .map(|row| row.iter().map(u8::to_string).collect::<Vec<_>>().join(", "))
    .collect();
  let expected = format!(
    "export const CONTRACT: Uint8Array = Uint8Array.from([\n  {},\n  {},\n]);",
    rows[0], rows[1]
  );
  assert!(code(&manifest).contains(&expected));
}

#[test]
fn imports_only_what_the_code_uses() {
  let code = code(&with_types(vec![("T", Definition::Alias(prim(Primitive::U8)))]));
  assert!(!code.contains("import"), "{code}");
  let code = generate(&Manifest::new("app").service(Service::new("s").server_method(Method::notify("n")))).unwrap();
  has(
    &code,
    "import { type Connection, type NotifyOptions, type NotifySignature, type Types } from \"@pavia/client\";",
  );
  assert!(generate(&chat()).unwrap().contains(
    "import {\n  type CallOptions,\n  type CallSignature,\n  type Connection,\n  type NotifyOptions,\n  type \
     NotifySignature,\n  StatusError,\n  type Types,\n} from \"@pavia/client\";"
  ));
}

#[test]
fn escapes_docs_so_they_cannot_end_the_comment() {
  let manifest =
    with_types(vec![("T", Definition::Alias(prim(Primitive::U8)))]).docs("A */ close\u{2028}then\u{2029}more\r\nlines");
  let code = code(&manifest);
  has(&code, "/**");
  has(&code, " * A *\\/ close");
  has(&code, " * then");
  has(&code, " * more");
  has(&code, " * lines");
  assert!(!code.contains('\u{2028}') && !code.contains('\u{2029}') && !code.contains('\r'));
}

#[test]
fn keeps_a_blank_line_of_docs_as_a_bare_star() {
  let manifest = with_types(vec![("T", Definition::Alias(prim(Primitive::U8)))]).docs("First.\n\nSecond.");
  let code = code(&manifest);
  let start = code.find("/**").expect("the doc comment");
  let doc: Vec<&str> = code[start..].lines().take(5).collect();
  assert_eq!(doc, ["/**", " * First.", " *", " * Second.", " */"]);
}

#[test]
fn refuses_names_typescript_reserves_or_the_generated_code_uses() {
  let reserved = |manifest: Manifest| match generate(&manifest) {
    Err(GenerateError::Reserved { name, .. }) => name,
    other => panic!("expected a reserved name, got {other:?}"),
  };
  let alias = |name: &str| with_types(vec![(name, Definition::Alias(prim(Primitive::U8)))]);
  for name in [
    "Promise",
    "Date",
    "Uint8Array",
    "Map",
    "StatusError",
    "Connection",
    "TYPES",
    "string",
    "class",
    "intrinsic",
    "__proto__",
    "constructor",
    "then",
  ] {
    assert_eq!(reserved(alias(name)), name);
  }
  let method = |name: &str| Manifest::new("app").service(Service::new("s").server_method(Method::notify(name)));
  for name in ["then", "constructor", "__proto__", "toString"] {
    assert_eq!(reserved(method(name)), name);
  }
  let param = |name: &str| {
    Manifest::new("app")
      .service(Service::new("s").server_method(Method::notify("n").param(Param::new(name, prim(Primitive::U8)))))
  };
  for name in ["class", "options", "error", "items", "CALLS", "eval"] {
    assert_eq!(reserved(param(name)), name);
  }
  let fields = |name: &str| Struct::new(vec![Field::new(name, prim(Primitive::U8))]);
  for name in ["__proto__", "constructor", "prototype", "then", "toString"] {
    assert_eq!(reserved(with_types(vec![("T", Definition::Struct(fields(name)))])), name);
    let inline = Manifest::new("app")
      .service(Service::new("s").server_method(Method::notify("n").param(Param::new("p", Type::Struct(fields(name))))));
    assert_eq!(reserved(inline), name);
  }
  assert_eq!(reserved(method("prototype")), "prototype");
  let variant = with_types(vec![("T", Definition::Enum(Enum::new(vec![Variant::unit("__proto__")])))]);
  assert!(generate(&variant).is_ok());
}

#[test]
fn refuses_two_parts_that_would_get_one_name() {
  let collision = |manifest: Manifest| match generate(&manifest) {
    Err(GenerateError::Collision { name, .. }) => name,
    other => panic!("expected a collision, got {other:?}"),
  };
  let services = Manifest::new("app")
    .service(Service::new("a_b").server_method(Method::notify("n")))
    .service(Service::new("aB").server_method(Method::notify("n")));
  assert_eq!(collision(services), "ABClient");
  let input = with_types(vec![
    ("Status", Definition::Enum(Enum::new(vec![Variant::unit("Idle")]).open())),
    ("StatusInput", Definition::Alias(prim(Primitive::U8))),
  ]);
  assert_eq!(collision(input), "StatusInput");
  let global = Manifest::new("app").service(Service::new("_").server_method(Method::call(
    "_",
    Call::unary(prim(Primitive::U8)).error(prim(Primitive::String)),
  )));
  assert_eq!(collision(global), "Error");
}

#[test]
fn gives_the_input_form_to_every_type_on_a_cycle_that_reaches_an_open_enum() {
  let code = code(&with_types(vec![
    ("Status", Definition::Enum(Enum::new(vec![Variant::unit("Idle")]).open())),
    (
      "Node",
      Definition::Struct(Struct::new(vec![Field::new(
        "next",
        Type::Option(Box::new(reference("Edge"))),
      )])),
    ),
    (
      "Edge",
      Definition::Struct(Struct::new(vec![
        Field::new("to", Type::Option(Box::new(reference("Node")))),
        Field::new("status", reference("Status")),
      ])),
    ),
    (
      "Loop",
      Definition::Struct(Struct::new(vec![Field::new(
        "again",
        Type::Option(Box::new(reference("Loop"))),
      )])),
    ),
  ]));
  has(&code, "  next: EdgeInput | null;");
  has(&code, "  to: NodeInput | null;");
  assert!(!code.contains("LoopInput"), "{code}");
}

#[test]
fn starts_a_derived_class_name_with_an_underscore_when_its_service_name_gives_a_digit() {
  let service = Service::new("_2fa")
    .server_method(Method::call(
      "verify",
      Call::unary(prim(Primitive::Bool)).error(prim(Primitive::String)),
    ))
    .client_method(Method::notify("prompt"));
  let code = code(&Manifest::new("app").service(service));
  has(&code, "export class _2faClient {");
  has(&code, "export class _2faVerifyError extends StatusError {");
  has(&code, "export interface _2faHandlers {");
}

#[test]
fn quotes_a_handler_named_new_so_it_stays_a_method() {
  let service = Service::new("s").client_method(Method::notify("new").param(Param::new("x", prim(Primitive::String))));
  let code = code(&Manifest::new("app").service(service));
  has(&code, "  \"new\"(x: string): void | Promise<void>;");
}

#[test]
fn refuses_a_type_that_is_an_option_of_itself() {
  let option = |ty: Type| Type::Option(Box::new(ty));
  let cycle = |types: Vec<(&str, Definition)>| match generate(&with_types(types)) {
    Err(GenerateError::OptionCycle { name, .. }) => name,
    other => panic!("expected an option cycle, got {other:?}"),
  };
  assert_eq!(cycle(vec![("A", Definition::Alias(option(reference("A"))))]), "A");
  assert_eq!(
    cycle(vec![
      ("B", Definition::Alias(option(option(reference("C"))))),
      ("C", Definition::Alias(reference("B"))),
    ]),
    "B"
  );
  let fine = with_types(vec![
    ("L", Definition::Alias(option(Type::List(Box::new(reference("L")))))),
    (
      "S",
      Definition::Struct(Struct::new(vec![Field::new("next", option(reference("T")))])),
    ),
    ("T", Definition::Alias(option(reference("S")))),
  ]);
  let code = code(&fine);
  has(&code, "export type L = L[] | null;");
  has(&code, "export type T = S | null;");
}

#[test]
fn types_what_the_client_sends_in_the_input_form_and_what_it_receives_as_decoded() {
  let status = || reference("Status");
  let service = Service::new("s")
    .server_method(Method::call("set", Call::unary(status()).error(status())).param(Param::new("value", status())))
    .server_method(Method::notify("tell").param(Param::new("value", status())))
    .server_method(Method::call("push", Call::client_stream(status(), status())))
    .server_method(Method::call("watch", Call::server_stream(status())))
    .client_method(Method::call("ask", Call::unary(status())).param(Param::new("value", status())))
    .client_method(Method::call("feed", Call::bidi(status(), status())));
  let manifest = Manifest::new("app").service(service).types(BTreeMap::from([(
    "Status".into(),
    Definition::Enum(Enum::new(vec![Variant::unit("Idle")]).open()),
  )]));
  let code = code(&manifest);
  for line in [
    "  async set(value: StatusInput, options?: CallOptions): Promise<Status> {",
    "  declare readonly detail: Status | undefined;",
    "  tell(value: StatusInput, options?: NotifyOptions): void {",
    "  push(items: AsyncIterable<StatusInput>, options?: CallOptions): Promise<Status> {",
    "  watch(options?: CallOptions): AsyncIterable<Status> {",
    "  ask(value: Status): StatusInput | Promise<StatusInput>;",
    "  feed(items: AsyncIterable<Status>): AsyncIterable<StatusInput>;",
  ] {
    has(&code, line);
  }
}

#[test]
fn refuses_a_manifest_that_does_not_validate() {
  let manifest = with_types(vec![("T", Definition::Alias(reference("Missing")))]);
  assert!(matches!(generate(&manifest), Err(GenerateError::Invalid(_))));
}

#[test]
fn gives_input_forms_along_a_long_chain_of_aliases_in_one_walk() {
  let count = 50_000;
  let mut types: Vec<(String, Definition)> = (0..count - 1)
    .map(|index| {
      (
        format!("A{index:05}"),
        Definition::Alias(reference(&format!("A{:05}", index + 1))),
      )
    })
    .collect();
  types.push((
    format!("A{:05}", count - 1),
    Definition::Enum(Enum::new(vec![Variant::unit("Idle")]).open()),
  ));
  let manifest = Manifest::new("app").types(types.into_iter().collect());
  let code = code(&manifest);
  has(&code, "export type A00000Input = A00001Input;");
  has(&code, &format!("export type A{:05}Input = {{ kind: \"Idle\" }};", count - 1));
}

#[test]
fn names_a_form_it_does_not_know_by_its_manifest_json() {
  let error = crate::front::unsupported(&serde_json::json!({"set": "u8"}));
  assert_eq!(
    error,
    GenerateError::Unsupported {
      form: r#"{"set":"u8"}"#.into()
    }
  );
  assert_eq!(error.to_string(), r#"{"set":"u8"} is in a form this generator does not know"#);
}

#[test]
fn writes_no_client_class_for_a_service_without_server_methods() {
  let code = code(&Manifest::new("app").service(Service::new("s").client_method(Method::notify("show"))));
  assert!(!code.contains("class SClient"), "{code}");
  assert!(!code.contains("Connection"), "{code}");
  has(&code, "export interface SHandlers {");
}

#[test]
fn reads_the_connection_in_a_streaming_stub() {
  let service = Service::new("s").server_method(Method::call("follow", Call::server_stream(prim(Primitive::String))));
  let code = code(&Manifest::new("app").service(service));
  has(&code, "    void [this.#connection, options];");
}
