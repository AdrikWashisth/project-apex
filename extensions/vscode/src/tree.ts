/**
 * Tree views for tasks and for a task's plan and subtasks.
 *
 * Both are read-only projections of runtime state. They hold nothing
 * themselves: every refresh re-queries the runtime, so the view is correct even
 * if the task was started from the CLI, the TUI or another window.
 */

import * as vscode from "vscode";
import { ApexClient } from "./client";

interface TaskLike {
  id: string;
  objective: string;
  status: string;
  mode: string;
  tool_calls: number;
  steps: number;
}

interface SubtaskLike {
  id: string;
  agent_id: string;
  status: string;
  wave: number;
  result?: string | null;
  error?: string | null;
}

/** The task list shown in the activity bar. */
export class TasksTree implements vscode.TreeDataProvider<TaskItem> {
  private readonly onDidChange = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.onDidChange.event;

  private tasks: TaskLike[] = [];

  constructor(private readonly client: ApexClient) {}

  refresh(): void {
    void this.load();
  }

  private async load(): Promise<void> {
    try {
      const response = (await this.client.request({ op: "list_tasks", limit: 100 })) as {
        op: string;
        tasks?: TaskLike[];
      };
      if (response.op === "task_list") {
        this.tasks = response.tasks ?? [];
        this.onDidChange.fire();
      }
    } catch (error) {
      console.warn("[apex] could not refresh tasks", error);
    }
  }

  getTreeItem(element: TaskItem): vscode.TreeItem {
    return element;
  }

  getChildren(): TaskItem[] {
    return this.tasks.map((task) => new TaskItem(task));
  }
}

class TaskItem extends vscode.TreeItem {
  constructor(task: TaskLike) {
    const label = task.objective.length > 60 ? `${task.objective.slice(0, 60)}…` : task.objective;
    super(label, vscode.TreeItemCollapsibleState.None);
    this.id = task.id;
    this.description = `${statusSymbol(task.status)} ${task.tool_calls} tool call(s), ${task.steps} step(s)`;
    this.tooltip = `${task.objective}\n\n${task.id}\nstatus: ${task.status}\nmode: ${task.mode}`;
    this.contextValue = task.status === "running" || task.status === "verifying"
      ? "apex.task.running"
      : "apex.task.finished";
    this.iconPath = statusIcon(task.status);
    this.command = {
      command: "apex.showPlan",
      title: "Show plan",
      arguments: [task.id],
    };
  }
}

/** The plan and subtask view for the selected task. */
export class PlanTree implements vscode.TreeDataProvider<PlanNode> {
  private readonly onDidChange = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.onDidChange.event;

  private subtasks: SubtaskLike[] = [];
  private waves: number[] = [];

  constructor(private readonly client: ApexClient) {}

  refresh(taskId: string): void {
    void this.load(taskId);
  }

  clear(): void {
    this.subtasks = [];
    this.waves = [];
    this.onDidChange.fire();
  }

  private async load(taskId: string): Promise<void> {
    try {
      const [subtaskResponse, planResponse] = await Promise.all([
        this.client.request({ op: "list_subtasks", task_id: taskId }) as Promise<{
          op: string;
          subtasks?: SubtaskLike[];
        }>,
        this.client.request({ op: "show_plan", task_id: taskId }) as Promise<{
          op: string;
          waves?: number[];
        }>,
      ]);
      if (subtaskResponse.op === "subtask_list") {
        this.subtasks = subtaskResponse.subtasks ?? [];
      }
      if (planResponse.op === "plan") {
        this.waves = planResponse.waves ?? [];
      }
      this.onDidChange.fire();
    } catch (error) {
      console.warn("[apex] could not refresh plan", error);
    }
  }

  getTreeItem(element: PlanNode): vscode.TreeItem {
    return element;
  }

  getChildren(): PlanNode[] {
    if (this.subtasks.length === 0) {
      const empty = new PlanNode("(no multi-step plan — single-agent task)", "", "");
      empty.contextValue = "apex.plan.empty";
      return [empty];
    }

    const nodes: PlanNode[] = [];
    let currentWave = -1;
    for (let index = 0; index < this.subtasks.length; index++) {
      const subtask = this.subtasks[index];
      if (!subtask) {
        continue;
      }
      const wave = this.waves[index] ?? 0;
      if (wave !== currentWave) {
        currentWave = wave;
        const header = new PlanNode(`Wave ${wave}`, "", "");
        header.contextValue = "apex.plan.wave";
        nodes.push(header);
      }
      const label = `${subtask.agent_id} — ${subtask.id}`;
      const detail = subtask.result ?? subtask.error ?? "";
      const node = new PlanNode(label, subtask.status, detail);
      node.contextValue = "apex.plan.subtask";
      nodes.push(node);
    }
    return nodes;
  }
}

class PlanNode extends vscode.TreeItem {
  constructor(label: string, description: string, detail: string) {
    super(label, vscode.TreeItemCollapsibleState.None);
    this.description = description;
    this.tooltip = detail ? `${label}\n\n${detail}` : label;
  }
}

// ------------------------------------------------------------------ status art

function statusSymbol(status: string): string {
  switch (status) {
    case "completed":
      return "✓";
    case "failed":
      return "✗";
    case "cancelled":
      return "⊘";
    case "running":
      return "●";
    case "verifying":
      return "◐";
    default:
      return "○";
  }
}

function statusIcon(status: string): vscode.ThemeIcon {
  switch (status) {
    case "completed":
      return new vscode.ThemeIcon("pass", new vscode.ThemeColor("testing.iconPassed"));
    case "failed":
      return new vscode.ThemeIcon("error", new vscode.ThemeColor("testing.iconFailed"));
    case "running":
    case "verifying":
      return new vscode.ThemeIcon("sync~spin");
    default:
      return new vscode.ThemeIcon("circle-outline");
  }
}

/** Format a timestamp from an event for display. */
export function formatTime(timestamp: string): string {
  return timestamp.length >= 19 ? timestamp.slice(11, 19) : timestamp;
}