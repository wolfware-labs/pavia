/** A primitive contract type (spec 12.1). */
export type Primitive =
  | "unit"
  | "bool"
  | "i8"
  | "i16"
  | "i32"
  | "i64"
  | "u8"
  | "u16"
  | "u32"
  | "u64"
  | "f32"
  | "f64"
  | "string"
  | "bytes"
  | "timestamp"
  | "uuid"
  | "any";

/**
 * A contract type in manifest syntax (spec 12.2): `"u64"`, `{ list: "string" }`, `{ ref: "Message" }`,
 * or a `{ struct }` written in place, as an enum variant's payload is.
 */
export type ContractType =
  | Primitive
  | { readonly option: ContractType }
  | { readonly list: ContractType }
  | { readonly map: ContractType }
  | { readonly tuple: readonly ContractType[] }
  | { readonly struct: { readonly fields: readonly Field[] } }
  | { readonly ref: string };

/** A struct field. `optional` fields may be left out on the wire; their type is an `option`. */
export interface Field {
  readonly name: string;
  readonly type: ContractType;
  readonly optional?: boolean;
  readonly docs?: string;
}

/** An enum variant; one without `type` is a unit variant. */
export interface Variant {
  readonly name: string;
  readonly type?: ContractType;
  readonly docs?: string;
}

/** A named type of the manifest's `types` table. */
export type NamedType =
  | { readonly struct: { readonly fields: readonly Field[] }; readonly docs?: string }
  | { readonly enum: { readonly variants: readonly Variant[]; readonly open?: boolean }; readonly docs?: string }
  | { readonly alias: ContractType; readonly docs?: string };

/** The manifest's `types` table, by name. */
export type Types = Readonly<Record<string, NamedType>>;

/**
 * An enum value: a unit variant is `{ kind }`, a data variant `{ kind, value }`. A variant an open
 * enum does not declare decodes as `{ kind: "Unknown", name }`, with its raw name (spec 12.1).
 */
export type EnumValue =
  | { readonly kind: string; readonly value?: unknown }
  | { readonly kind: "Unknown"; readonly name: string };
