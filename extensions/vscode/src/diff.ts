/**
 * Diff viewing.
 *
 * Uses the editor's own diff renderer rather than re-implementing one, and always
 * shows the runtime's current diff for a task's project.
 */

import * as vscode from "vscode";
import { ApexClient } from "./client";

/** Show the runtime's diff for a task in the editor's diff view. */
export async function showDiff(client: ApexClient, taskId: string): Promise<void> {
  let response: { op: string; diff?: string };
  try {
    response = (await client.request({ op: "get_diff", task_id: taskId })) as {
      op: string;
      diff?: string;
    };
  } catch (error) {
    void vscode.window.showErrorMessage(`APEX: could not load the diff — ${String(error)}`);
    return;
  }

  if (response.op !== "diff" || response.diff === undefined) {
    void vscode.window.showInformationMessage("APEX: this task has no diff.");
    return;
  }

  const content = response.diff;
  const title = `APEX diff — ${taskId}`;

  // Write to a scratch document so the editor can syntax-highlight it.
  const document = await vscode.workspace.openTextDocument({
    language: "diff",
    content,
  });
  await vscode.window.showTextDocument(document, { preview: false });
  void vscode.window.setStatusBarMessage(title, 4000);
}

/** Run verification for a task and report the outcome. */
export async function verify(client: ApexClient, taskId: string): Promise<void> {
  let response: { op: string; passed?: boolean; checks?: Array<{ name: string; passed: boolean; detail: string }> };
  try {
    response = (await client.request({ op: "verify", task_id: taskId })) as {
      op: string;
      passed?: boolean;
      checks?: Array<{ name: string; passed: boolean; detail: string }>;
    };
  } catch (error) {
    void vscode.window.showErrorMessage(`APEX: verification failed to run — ${String(error)}`);
    return;
  }

  if (response.op !== "verification" || !response.checks) {
    void vscode.window.showInformationMessage("APEX: no verification result.");
    return;
  }

  const lines = response.checks.map(
    (check) => `${check.passed ? "PASS" : "FAIL"}  ${check.name}\n${indent(check.detail, 2)}`
  );
  const summary = response.passed ? "Verification passed" : "Verification failed";
  const channel = vscode.window.createOutputChannel("APEX Verification");
  channel.appendLine(`${summary} — ${response.checks.length} check(s)`);
  channel.appendLine(lines.join("\n\n"));
  channel.show(true);
}

function indent(text: string, spaces: number): string {
  const pad = " ".repeat(spaces);
  return text
    .split("\n")
    .map((line) => (line ? `${pad}${line}` : line))
    .join("\n");
}