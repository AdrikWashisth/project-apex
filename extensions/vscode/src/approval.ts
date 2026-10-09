/**
 * Approval prompts.
 *
 * A gated action (a risky tool call) arrives from the runtime as an event. The
 * extension surfaces it as a modal prompt and translates the answer into a
 * protocol message. Nobody answering before the timeout counts as a denial, so
 * an unattended session fails closed.
 */

import * as vscode from "vscode";
import { ApexClient } from "./client";

interface ApprovalRequested {
  type: string;
  approval_id: string;
  action: string;
  risk: string;
  detail: string;
}

/** Pending approvals awaiting a human decision. */
export class ApprovalProvider {
  private pending = new Map<string, (approved: boolean) => void>();

  constructor(private readonly client: ApexClient) {}

  /** Wire the runtime's approval events into prompts. */
  start(): void {
    this.client.on("event", (event) => {
      const kind = event as ApprovalRequested;
      if (kind.type !== "approval_requested") {
        return;
      }
      void this.prompt(kind);
    });
  }

  private async prompt(request: ApprovalRequested): Promise<void> {
    const approved = await vscode.window.showWarningMessage(
      `${request.action}  (risk: ${request.risk})\n\n${truncate(request.detail, 400)}`,
      { modal: true },
      "Approve",
      "Deny"
    );

    const decision = approved === "Approve";
    try {
      await this.client.request({
        op: "resolve_approval",
        task_id: "",
        approval_id: request.approval_id,
        approved: decision,
      });
    } catch (error) {
      console.warn("[apex] could not deliver the approval decision", error);
    }
  }

  /** Stop listening for approval requests. */
  stop(): void {
    this.client.off("event", this.prompt);
    this.pending.clear();
  }
}

function truncate(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max)}…`;
}