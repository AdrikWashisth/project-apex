/**
 * APEX runtime client for VS Code.
 *
 * This is a pure frontend: it speaks the versioned wire protocol and renders
 * what the runtime reports. It contains no agent logic, no task store and no
 * model calls — the runtime owns all of that.
 *
 * Frames are newline-delimited JSON tagged by a `frame` field:
 *   request  { frame: "request",  id, request  }
 *   response { frame: "response", id, response }
 *   event    { frame: "event",    event    }
 */

import * as net from "net";
import { EventEmitter } from "events";

export interface ConnectionInfo {
  transport: "local_socket" | "tcp_loopback";
  endpoint: string;
  token: string;
}

/** Raised when the connection is established and the handshake succeeded. */
export interface HandshakeResult {
  protocolVersion: number;
  runtimeVersion: string;
  sessionId: string;
}

type RequestEnvelope = { frame: "request"; id: number; request: unknown };
type ResponseEnvelope = { frame: "response"; id: number; response: unknown };
type EventEnvelope = { frame: "event"; event: unknown };
type Frame = RequestEnvelope | ResponseEnvelope | EventEnvelope;

type Pending = (response: unknown) => void;

/**
 * Connected APEX runtime client.
 *
 * Responses are matched to requests by id. Live events are emitted on the
 * `event` channel. If the connection drops, `disconnect` is raised and the
 * caller is expected to reconnect — this is deliberately not automatic, so a
 * dropped session is visible rather than silently retried.
 */
export class ApexClient extends EventEmitter {
  private socket: net.Socket | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private buffer = "";
  private closed = false;

  constructor(private readonly info: ConnectionInfo) {
    super();
  }

  get isConnected(): boolean {
    return this.socket !== null && !this.closed;
  }

  /** Connect and complete the handshake, which also validates the protocol. */
  connect(): Promise<HandshakeResult> {
    return new Promise((resolve, reject) => {
      const onError = (error: Error) => {
        this.cleanup();
        reject(error);
      };

      const socket = this.openSocket();
      this.socket = socket;
      socket.once("error", onError);
      socket.once("close", () => {
        this.failAll(new Error("connection to the APEX runtime closed"));
        this.cleanup();
        this.closed = true;
        this.emit("disconnect");
      });

      // Handshake is the first thing on the wire, so wait for a clean read.
      const onData = (chunk: Buffer) => {
        this.buffer += chunk.toString("utf8");
        const newline = this.buffer.indexOf("\n");
        if (newline < 0) {
          return;
        }
        const line = this.buffer.slice(0, newline);
        this.buffer = this.buffer.slice(newline + 1);
        socket.off("data", onData);
        socket.off("error", onError);

        let frame: Frame;
        try {
          frame = JSON.parse(line) as Frame;
        } catch (error) {
          this.cleanup();
          reject(new Error(`malformed handshake frame: ${String(error)}`));
          return;
        }
        if (frame.frame !== "response") {
          this.cleanup();
          reject(new Error("expected a handshake response"));
          return;
        }
        const response = frame.response as { op?: string; protocol_version?: number; runtime_version?: string; session_id?: string; message?: string };
        if (response.op === "error") {
          this.cleanup();
          reject(new Error(`handshake rejected: ${response.message ?? "unknown error"}`));
          return;
        }
        if (
          response.protocol_version !== PROTOCOL_VERSION ||
          typeof response.runtime_version !== "string" ||
          typeof response.session_id !== "string"
        ) {
          this.cleanup();
          reject(new Error("unexpected handshake response"));
          return;
        }
        this.attachReader(socket);
        resolve({
          protocolVersion: response.protocol_version,
          runtimeVersion: response.runtime_version,
          sessionId: response.session_id,
        });
      };
      socket.on("data", onData);
    });
  }

  /** Send a request and await its correlated response. */
  request(request: unknown): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const socket = this.socket;
      if (!socket || this.closed) {
        reject(new Error("not connected to an APEX runtime"));
        return;
      }
      const id = this.nextId++;
      this.pending.set(id, resolve);
      const payload = JSON.stringify({ frame: "request", id, request }) + "\n";
      socket.write(payload, (error) => {
        if (error) {
          this.pending.delete(id);
          reject(error);
        }
      });
    });
  }

  disconnect(): void {
    this.failAll(new Error("disconnected"));
    this.cleanup();
  }

  // ---------------------------------------------------------------- internals

  private openSocket(): net.Socket {
    if (this.info.transport === "local_socket") {
      // A Unix-domain socket path.
      return net.createConnection(this.info.endpoint);
    }
    const endpoint = this.info.endpoint;
    const separator = endpoint.lastIndexOf(":");
    const host = endpoint.slice(0, separator);
    const port = Number(endpoint.slice(separator + 1));
    return net.createConnection({ host, port });
  }

  /**
   * Attach the reader that runs for the rest of the connection's life.
   *
   * The handshake path reads inline so it can bail out before wiring this up.
   */
  private attachReader(socket: net.Socket): void {
    socket.on("data", (chunk: Buffer) => {
      this.buffer += chunk.toString("utf8");
      let newline = this.buffer.indexOf("\n");
      while (newline >= 0) {
        const line = this.buffer.slice(0, newline);
        this.buffer = this.buffer.slice(newline + 1);
        this.handleLine(line);
        newline = this.buffer.indexOf("\n");
      }
    });
  }

  private handleLine(line: string): void {
    let frame: Frame;
    try {
      frame = JSON.parse(line) as Frame;
    } catch (error) {
      console.warn("[apex] dropping malformed frame", error);
      return;
    }
    if (frame.frame === "response") {
      const resolve = this.pending.get(frame.id);
      if (resolve) {
        this.pending.delete(frame.id);
        resolve(frame.response);
      }
      return;
    }
    if (frame.frame === "event") {
      this.emit("event", frame.event);
    }
  }

  private failAll(error: Error): void {
    for (const resolve of this.pending.values()) {
      resolve({ op: "error", message: error.message });
    }
    this.pending.clear();
  }

  private cleanup(): void {
    if (this.socket) {
      this.socket.removeAllListeners();
      this.socket.destroy();
      this.socket = null;
    }
  }
}

/** Must match PROTOCOL_VERSION in crates/apex-protocol/src/wire.rs. */
export const PROTOCOL_VERSION = 1;

/** A request envelope, as sent on the wire. */
export function hello(token: string): unknown {
  return {
    op: "hello",
    protocol_version: PROTOCOL_VERSION,
    client_name: "apex-vscode",
    client_version: "0.1.0",
    token,
  };
}