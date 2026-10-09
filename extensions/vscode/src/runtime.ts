/**
 * Runtime discovery and lifecycle.
 *
 * The extension never starts agent logic of its own. If no runtime is running
 * it offers to start one (`apex runtime`), and if the protocol versions disagree
 * it says so instead of degrading into a partial mode.
 */

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { ExtensionContext, Uri, window, workspace } from "vscode";

import { ApexClient, ConnectionInfo, PROTOCOL_VERSION } from "./client";

/** Where the runtime publishes its connection info. */
export function runtimeInfoPath(): string {
  const home = process.env.APEX_HOME ?? path.join(os.homedir(), ".apex");
  return path.join(home, "state", "runtime.json");
}

/** Read the connection info published by a running runtime. */
export function readConnectionInfo(): ConnectionInfo | undefined {
  try {
    const raw = fs.readFileSync(runtimeInfoPath(), "utf8");
    const info = JSON.parse(raw) as ConnectionInfo;
    if (!info.endpoint || !info.token) {
      return undefined;
    }
    return info;
  } catch {
    return undefined;
  }
}

/** Whether the published info looks usable. */
export function hasRuntime(): boolean {
  return readConnectionInfo() !== undefined;
}

/**
 * Connect to the runtime, starting one first if necessary.
 *
 * Returns the client, or undefined if the user declined to start a runtime.
 */
export async function connectOrStart(
  context: ExtensionContext,
  output: { line: (text: string) => void }
): Promise<ApexClient | undefined> {
  const existing = readConnectionInfo();
  if (existing) {
    const client = await tryConnect(existing, output);
    if (client) {
      return client;
    }
    output.line("[apex] the runtime at the published address did not respond");
  }

  const start = await window.showInformationMessage(
    "No APEX runtime is running. Start one?",
    "Start",
    "Cancel"
  );
  if (start !== "Start") {
    return undefined;
  }

  const executable = workspace.getConfiguration("apex").get<string>("executable") ?? "apex";
  output.line(`[apex] starting the runtime with: ${executable} runtime`);

  const { spawn } = await import("child_process");
  const child = spawn(executable, ["runtime", "--foreground"], {
    detached: true,
    stdio: "ignore",
  });
  child.unref();

  // The runtime publishes its address once it is listening.
  for (let attempt = 0; attempt < 60; attempt++) {
    await sleep(250);
    const info = readConnectionInfo();
    if (info) {
      const client = await tryConnect(info, output);
      if (client) {
        return client;
      }
    }
  }

  window.showErrorMessage(
    "APEX: the runtime did not publish its address. Run `apex runtime` by hand and check `apex doctor`."
  );
  return undefined;
}

/** Attempt a connection, reporting the reason if it fails. */
async function tryConnect(
  info: ConnectionInfo,
  output: { line: (text: string) => void }
): Promise<ApexClient | undefined> {
  const client = new ApexClient(info);
  try {
    const handshake = await client.connect();
    output.line(
      `[apex] connected to runtime v${handshake.runtimeVersion} (protocol v${handshake.protocolVersion})`
    );
    if (handshake.protocolVersion !== PROTOCOL_VERSION) {
      window.showWarningMessage(
        `APEX: the runtime speaks protocol v${handshake.protocolVersion} but this extension ` +
          `speaks v${PROTOCOL_VERSION}. Update the extension or the CLI.`
      );
    }
    return client;
  } catch (error) {
    output.line(`[apex] could not connect: ${String(error)}`);
    client.disconnect();
    return undefined;
  }
}

/** Ask the user for a task objective. */
export async function promptForObjective(prompt: string): Promise<string | undefined> {
  return window.showInputBox({
    prompt,
    placeHolder: "Describe what APEX should do",
    ignoreFocusOut: true,
  });
}

/** Open a file in the editor, creating it if it does not exist. */
export async function openFile(uri: Uri): Promise<void> {
  const document = await workspace.openTextDocument(uri);
  await window.showTextDocument(document, { preview: false });
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
