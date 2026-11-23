import { toHex } from "./bytes.js";
import { CloseCode } from "./errors.js";
import type { Frame, Welcome } from "./frames.js";
import { ClientSession, type Ending, type SessionInput, type SessionOptions } from "./session.js";

/** The WebSocket subprotocol of this protocol version (spec 4.3). */
export const SUBPROTOCOL = "pavia.1";

/** The part of the WebSocket API the driver uses, as browsers and Node 22 provide it. */
export interface WebSocketLike {
  binaryType: string;
  readonly protocol: string;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { readonly data: unknown }) => void) | null;
  onclose: ((event: { readonly code: number }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  send(data: Uint8Array): void;
  close(code?: number, reason?: string): void;
}

/** A WebSocket constructor, `globalThis.WebSocket` by default. */
export type WebSocketConstructor = new (url: string, protocols: string | string[]) => WebSocketLike;

/** {@link SessionOptions} and what the driver needs besides. */
export interface ConnectOptions extends SessionOptions {
  /** The WebSocket constructor. Default `globalThis.WebSocket`. */
  readonly WebSocket?: WebSocketConstructor | undefined;
  /** A monotonic clock in milliseconds. Default `performance.now`. */
  readonly now?: (() => number) | undefined;
  /** Where the contract mismatch warning goes. Default `console.warn`. */
  readonly warn?: ((message: string) => void) | undefined;
}

/** The fingerprints of a contract mismatch (spec 7.2): this client's and the server's. */
export interface ContractMismatch {
  readonly local: Uint8Array;
  readonly remote: Uint8Array;
}

/** Why {@link connect} failed: the session ended before WELCOME, as `ending` says. */
export class ConnectError extends Error {
  override readonly name = "ConnectError";

  constructor(readonly ending: Ending) {
    super(`session ended before WELCOME: ${describeEnding(ending)}`);
  }
}

/**
 * Opens a session over WebSocket (spec 4.3, 7): requests the `pavia.1` subprotocol, sends HELLO
 * and resolves once WELCOME arrives. It rejects with {@link ConnectError} on REJECT, on CLOSE or a
 * lost transport before WELCOME, and on the handshake timeout.
 *
 * @throws {@link EncodeError} synchronously, before any socket opens, when the options make a HELLO
 * no server accepts, such as `max_frame` below 64 KiB.
 */
export function connect(url: string | URL, options: ConnectOptions = {}): Promise<Connection> {
  const now = options.now ?? (() => performance.now());
  const session = new ClientSession(options, now());
  const WebSocket = options.WebSocket ?? (globalThis as { WebSocket?: WebSocketConstructor }).WebSocket;
  if (WebSocket === undefined) throw new TypeError("no WebSocket implementation; pass options.WebSocket");
  const socket = new WebSocket(String(url), SUBPROTOCOL);
  return new Promise((resolve, reject) => {
    new Connection(socket, session, now, options.warn ?? ((message) => console.warn(message)), resolve, reject);
  });
}

/**
 * An established session over one WebSocket. It dispatches `contractMismatch` (a `CustomEvent`
 * with a {@link ContractMismatch}) once, right after {@link connect} resolves, when WELCOME
 * carries a fingerprint other than the client's, and `close` (a `CustomEvent` with the
 * {@link Ending}) when the session ends.
 */
export class Connection extends EventTarget {
  private timer: ReturnType<typeof setTimeout> | undefined;
  private welcomed: Welcome | undefined;
  private mismatch: ContractMismatch | undefined;
  private ending: Ending | undefined;
  private readonly closed: Promise<Ending>;
  private settleClosed: (ending: Ending) => void = () => {};

  /** @internal Built by {@link connect}. */
  constructor(
    private readonly socket: WebSocketLike,
    private readonly session: ClientSession,
    private readonly now: () => number,
    private readonly warn: (message: string) => void,
    private readonly resolveConnect: (connection: Connection) => void,
    private readonly rejectConnect: (error: ConnectError) => void,
  ) {
    super();
    this.closed = new Promise((resolve) => {
      this.settleClosed = resolve;
    });
    socket.binaryType = "arraybuffer";
    socket.onopen = () => this.opened();
    socket.onmessage = (event) => this.feed({ kind: "message", data: messageData(event.data) });
    socket.onclose = (event) => this.feed({ kind: "transport_closed", code: event.code });
    socket.onerror = () => {};
    this.arm();
  }

  /** The WELCOME the server sent; it stays readable after the session ends. */
  get welcome(): Welcome {
    return this.welcomed as Welcome;
  }

  /** The contract mismatch WELCOME showed, if any. */
  get contractMismatch(): ContractMismatch | undefined {
    return this.mismatch;
  }

  /** How the session ended, once it has. */
  get closedWith(): Ending | undefined {
    return this.ending;
  }

  /**
   * Sends CLOSE with `code` and `msg` and closes the WebSocket with `4000 + code` (spec 7.7). The
   * promise resolves with the session's {@link Ending} once it has ended.
   */
  close(code: CloseCode = CloseCode.NORMAL, msg?: string): Promise<Ending> {
    this.session.close(code, msg, this.now());
    this.drain();
    return this.closed;
  }

  /** @internal The next client stream ID, for the call layer. */
  openStream(): number {
    return this.session.openStream();
  }

  /** @internal Sends `frame` on `stream`, for the call layer; frames arrive as `frame` events. */
  send(stream: number, frame: Frame): void {
    this.session.send(stream, frame, this.now());
    this.drain();
  }

  private opened(): void {
    if (this.socket.protocol !== SUBPROTOCOL) {
      this.session.close(CloseCode.PROTOCOL_ERROR, "pavia.1 subprotocol not selected", this.now());
      return this.drain();
    }
    this.feed({ kind: "open" });
  }

  private feed(input: SessionInput): void {
    this.session.handleInput(input, this.now());
    this.drain();
  }

  private drain(): void {
    for (let output = this.session.pollOutput(); output !== undefined; output = this.session.pollOutput()) {
      switch (output.kind) {
        case "transmit":
          this.socket.send(output.data);
          break;
        case "close_transport":
          this.socket.close(output.code, output.reason);
          break;
        case "contract_mismatch":
          this.mismatch = { local: output.local, remote: output.remote };
          this.warn(`pavia: contract mismatch, client ${toHex(output.local)}, server ${toHex(output.remote)}`);
          break;
        case "established":
          this.welcomed = output.welcome;
          this.resolveConnect(this);
          if (this.mismatch !== undefined) {
            const detail = this.mismatch;
            setTimeout(() => this.dispatchEvent(new CustomEvent("contractMismatch", { detail })), 0);
          }
          break;
        case "frame":
          this.dispatchEvent(new CustomEvent("frame", { detail: { stream: output.stream, frame: output.frame } }));
          break;
        case "ended":
          this.ending = output.ending;
          if (this.welcomed === undefined) this.rejectConnect(new ConnectError(output.ending));
          this.settleClosed(output.ending);
          this.dispatchEvent(new CustomEvent("close", { detail: output.ending }));
          break;
      }
    }
    this.arm();
  }

  private arm(): void {
    clearTimeout(this.timer);
    const at = this.session.pollTimeout();
    if (at === undefined) return;
    this.timer = setTimeout(() => this.feed({ kind: "timeout" }), Math.max(0, at - this.now()));
  }
}

function messageData(data: unknown): Uint8Array | ArrayBuffer | string {
  if (typeof data === "string" || data instanceof ArrayBuffer || ArrayBuffer.isView(data)) {
    return ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : data;
  }
  return "";
}

function describeEnding(ending: Ending): string {
  switch (ending.kind) {
    case "closed":
      return `closed by the ${ending.by === "local" ? "client" : "server"} with code ${ending.code}`;
    case "rejected":
      return `rejected with code ${ending.code}: ${JSON.stringify(ending.msg)}`;
    case "transport_lost":
      return "the transport was lost";
  }
}
