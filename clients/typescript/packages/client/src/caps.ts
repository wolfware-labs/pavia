import type { Frame, Sequencing } from "./frames.js";

/**
 * The capability names of spec 7.10. `calls` is always granted, whatever WELCOME lists.
 */
export type Capability =
  | "calls"
  | "server-calls"
  | "streams"
  | "resume"
  | "channels"
  | "history"
  | "presence"
  | "datagrams"
  | "lanes"
  | "idempotency";

/** The capabilities a session may use: those WELCOME granted, and `calls`. */
export function grantedCaps(welcomeCaps: readonly string[]): ReadonlySet<string> {
  return new Set(["calls", ...welcomeCaps]);
}

/**
 * Whether a frame the server sent uses only capabilities in `granted` (spec 7.10), as
 * `pavia-proto`'s `caps::check` decides for a frame whose sender is the server.
 */
export function serverFrameAllowed(granted: ReadonlySet<string>, frame: Frame): boolean {
  return needed(frame).every((capability) => granted.has(capability));
}

function resumeOf(sequencing: Sequencing): Capability[] {
  return sequencing.seq !== undefined || sequencing.ack !== undefined ? ["resume"] : [];
}

function needed(frame: Frame): Capability[] {
  switch (frame.type) {
    case "CALL":
      return ["calls", "server-calls", ...(frame.shape >= 1n && frame.shape <= 3n ? (["streams"] as const) : [])];
    case "ITEM":
    case "END":
    case "RESULT":
    case "ERROR":
    case "CANCEL":
      return ["calls"];
    case "NOTIFY":
      return ["calls", ...resumeOf(frame)];
    case "ACK":
      return ["resume"];
    case "SUBSCRIBE":
      return [
        "channels",
        ...(frame.since === undefined ? [] : (["history"] as const)),
        ...(frame.presence === undefined ? [] : (["presence"] as const)),
        ...resumeOf(frame),
      ];
    case "SUBSCRIBED":
    case "UNSUBSCRIBE":
    case "UNSUBSCRIBED":
    case "PUBLICATION":
      return ["channels", ...resumeOf(frame)];
    case "PRESENCE":
      return ["presence", ...resumeOf(frame)];
    case "DATAGRAM":
      return ["datagrams"];
    case "DGRAM_BIND":
      return ["datagrams", ...resumeOf(frame)];
    case "LANE_OPEN":
      return ["lanes"];
    case "AUTH_REFRESH":
    case "AUTH_RESULT":
    case "AUTH_EXPIRING":
    case "REQUEST_ERROR":
      return resumeOf(frame);
    case "HELLO":
    case "WELCOME":
    case "REJECT":
    case "PING":
    case "PONG":
    case "GOAWAY":
    case "CLOSE":
    case "CREDIT":
    case "EXTENSION":
      return [];
  }
}
