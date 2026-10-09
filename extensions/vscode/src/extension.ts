/**
 * APEX VS Code extension.
 *
 * A frontend for the local APEX runtime. It discovers the runtime, checks
 * protocol compatibility, renders tasks, plans, subtasks and diffs, and turns
 * user actions into protocol messages.
 *
 * It never reimplements agent logic. Every action is a message on the wire and
 * every piece of state comes from the runtime, so a task started here is the
 * same task you can watch, resume or cancel from the CLI or the TUI.
 */

import * as vscode from "vscode";
import type { ExtensionContext } from "vscode";
import { ApexClient } from "./client";
import { ApprovalProvider } from "./approval";
import { showDiff, verify } from "./diff";
import { connectOrStart, promptForObjective } from "./runtime";
import { PlanTree, TasksTree } from "./tree";

export async function activate(context: ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel("APEX");
  const line = (text: string): void => {
    output.appendLine(text);
  };

  let client: ApexClient | undefined;
  let tasksTree: TasksTree | undefined;
  let planTree: PlanTree | undefined;
  let approvals: ApprovalProvider | undefined;
  let selectedTaskId: string | undefined;
  let timer: NodeJS.Timeout | undefined;

  /** Register the views that depend on a live connection. */
  const registerViews = (): void => {
    if (!client) {
      return;
    }
    tasksTree = new TasksTree(client);
    planTree = new PlanTree(client);
    approvals = new ApprovalProvider(client);

    context.subscriptions.push(
      vscode.window.registerTreeDataProvider("apex.tasks", tasksTree)
    );
    context.subscriptions.push(
      vscode.window.registerTreeDataProvider("apex.plan", planTree)
    );

    approvals.start();
    void tasksTree.refresh();

    const interval = vscode.workspace
      .getConfiguration("apex")
      .get<number>("refreshIntervalMs", 1500);
    timer = setInterval(() => {
      tasksTree?.refresh();
      if (selectedTaskId) {
        planTree?.refresh(selectedTaskId);
      }
    }, Math.max(250, interval));
  };

  /** Tear everything down so reconnecting starts clean. */
  const teardown = (): void => {
    if (timer) {
      clearInterval(timer);
      timer = undefined;
    }
    approvals?.stop();
    approvals = undefined;
    tasksTree = undefined;
    planTree = undefined;
    selectedTaskId = undefined;
  };

  const connect = async (): Promise<void> => {
    teardown();
    client = await connectOrStart(context, { line });
    if (client) {
      registerViews();
    } else {
      void vscode.window.showInformationMessage("APEX: not connected to a runtime.");
    }
  };

  context.subscriptions.push(
    vscode.commands.registerCommand("apex.connect", () => void connect()),
    vscode.commands.registerCommand("apex.refresh", () => {
      tasksTree?.refresh();
      if (selectedTaskId) {
        planTree?.refresh(selectedTaskId);
      }
    }),
    vscode.commands.registerCommand("apex.newTask", async () => {
      if (!client) {
        void vscode.window.showWarningMessage("APEX: connect to a runtime first.");
        return;
      }
      const objective = await promptForObjective("What should APEX do?");
      if (!objective) {
        return;
      }
      try {
        await client.request({
          op: "create_task",
          objective,
          project_root: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? "",
          model: undefined,
          agent_id: undefined,
          mode: "single",
          team: undefined,
          plan: undefined,
          workflow: undefined,
        });
        tasksTree?.refresh();
      } catch (error) {
        void vscode.window.showErrorMessage(`APEX: could not create the task — ${String(error)}`);
      }
    }),
    vscode.commands.registerCommand("apex.resumeTask", async (taskId: string) => {
      if (!client) {
        return;
      }
      try {
        await client.request({ op: "resume_task", task_id: taskId });
        tasksTree?.refresh();
      } catch (error) {
        void vscode.window.showErrorMessage(`APEX: could not resume the task — ${String(error)}`);
      }
    }),
    vscode.commands.registerCommand("apex.cancelTask", async (taskId: string) => {
      if (!client) {
        return;
      }
      try {
        await client.request({ op: "cancel_task", task_id: taskId });
        tasksTree?.refresh();
      } catch (error) {
        void vscode.window.showErrorMessage(`APEX: could not cancel the task — ${String(error)}`);
      }
    }),
    vscode.commands.registerCommand("apex.showDiff", async (taskId: string) => {
      if (client) {
        await showDiff(client, taskId);
      }
    }),
    vscode.commands.registerCommand("apex.showPlan", async (taskId: string) => {
      selectedTaskId = taskId;
      planTree?.refresh(taskId);
      await vscode.commands.executeCommand("apex.plan.focus");
    }),
    vscode.commands.registerCommand("apex.verify", async (taskId: string) => {
      if (client) {
        await verify(client, taskId);
      }
    })
  );

  if (vscode.workspace.getConfiguration("apex").get<boolean>("autoConnect", true)) {
    await connect();
  }
}

export function deactivate(): void {
  // The runtime keeps running; tasks outlive this window by design.
}