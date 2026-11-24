import {
  type ConnectOptions,
  connect,
  DEFAULT_MAX_FRAME,
  decodeFrame,
  decodeMessage,
  type Frame,
  type WebSocketLike,
  type Welcome,
} from "../src/index.js";
import { arrayBufferOf, message, welcome } from "./support.js";

export class FakeSocket implements WebSocketLike {
  static last: FakeSocket | undefined;
  static count = 0;
  binaryType = "blob";
  protocol = "";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { readonly data: unknown }) => void) | null = null;
  onclose: ((event: { readonly code: number }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  readonly sent: Frame[] = [];
  readonly sentOn: { readonly stream: number; readonly frame: Frame }[] = [];
  closedWith: { code: number | undefined; reason: string | undefined } | undefined;

  constructor(
    readonly url: string,
    readonly protocols: string | string[],
  ) {
    FakeSocket.last = this;
    FakeSocket.count++;
  }

  send(data: Uint8Array): void {
    for (const { stream, frame } of decodeMessage(data, DEFAULT_MAX_FRAME)) {
      const decoded = decodeFrame(frame);
      this.sent.push(decoded);
      this.sentOn.push({ stream, frame: decoded });
    }
  }

  close(code?: number, reason?: string): void {
    this.closedWith = { code, reason };
  }

  open(protocol = "pavia.1"): void {
    this.protocol = protocol;
    this.onopen?.({});
  }

  receive(frame: Frame, stream = 0): void {
    this.onmessage?.({ data: arrayBufferOf(message(frame, stream)) });
  }

  drop(code: number): void {
    this.onclose?.({ code });
  }
}

export function options(extra: Partial<ConnectOptions> = {}): ConnectOptions {
  return { WebSocket: FakeSocket, now: () => Date.now(), warn: () => {}, ...extra };
}

export function socket(): FakeSocket {
  return FakeSocket.last as FakeSocket;
}

export async function established(extra: Partial<ConnectOptions> = {}, fields: Partial<Welcome> = {}) {
  const pending = connect("wss://example.test/rt", options(extra));
  socket().open();
  socket().receive(welcome(fields));
  return { connection: await pending, ws: socket() };
}
