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
    (Type::Struct(Struct::new(Vec::new())), "{}"),
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
  for name in ["class", "options", "error", "CALLS", "eval"] {
    assert_eq!(reserved(param(name)), name);
  }
  let field = with_types(vec![(
    "T",
    Definition::Struct(Struct::new(vec![Field::new("__proto__", prim(Primitive::U8))])),
  )]);
  assert_eq!(reserved(field), "__proto__");
}

#[test]
fn refuses_two_parts_that_would_get_one_name() {
  let collision = |manifest: Manifest| match generate(&manifest) {
    Err(GenerateError::Collision { name, .. }) => name,
    other => panic!("expected a collision, got {other:?}"),
  };
  let services = Manifest::new("app").service(Service::new("a_b")).service(Service::new("aB"));
  assert_eq!(collision(services), "ABClient");
  let input = with_types(vec![
    ("Status", Definition::Enum(Enum::new(vec![Variant::unit("Idle")]).open())),
    ("StatusInput", Definition::Alias(prim(Primitive::U8))),
  ]);
  assert_eq!(collision(input), "StatusInput");
}

#[test]
fn refuses_a_manifest_that_does_not_validate() {
  let manifest = with_types(vec![("T", Definition::Alias(reference("Missing")))]);
  assert!(matches!(generate(&manifest), Err(GenerateError::Invalid(_))));
}
