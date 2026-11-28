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

/**
 * A struct field. `optional` fields may be left out on the wire; their type is an `option`.
 *
 * `name` is the field's name on the wire. `local`, when present, is the name the application's
 * values use for it instead, as code generated with a naming policy declares them (spec 12.3):
 * the codec reads and writes the value's field by `local` and the wire's by `name`. A manifest
 * never has `local`; the generator adds it.
 */
export interface Field {
  readonly name: string;
  readonly local?: string | undefined;
  readonly type: ContractType;
  readonly optional?: boolean | undefined;
  readonly docs?: string | undefined;
}

/** An enum variant; one without `type` is a unit variant. */
export interface Variant {
  readonly name: string;
  readonly type?: ContractType | undefined;
  readonly docs?: string | undefined;
}

/** A named type of the manifest's `types` table. */
export type NamedType =
  | { readonly struct: { readonly fields: readonly Field[] }; readonly docs?: string | undefined }
  | {
      readonly enum: { readonly variants: readonly Variant[]; readonly open?: boolean | undefined };
      readonly docs?: string | undefined;
    }
  | { readonly alias: ContractType; readonly docs?: string | undefined };

/** The manifest's `types` table, by name. */
export type Types = Readonly<Record<string, NamedType>>;

/**
 * An enum value: a unit variant is `{ kind }`, a data variant `{ kind, value }`. A variant an open
 * enum does not declare decodes as `{ kind: "Unknown", name }`, with its raw name (spec 12.1).
 */
export type EnumValue =
  | { readonly kind: string; readonly value?: unknown }
  | { readonly kind: "Unknown"; readonly name: string };
