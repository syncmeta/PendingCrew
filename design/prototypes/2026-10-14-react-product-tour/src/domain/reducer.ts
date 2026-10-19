import { validDataView } from "./dataViews";
import { refreshDocumentLibrary } from "./documentLibrary";
import { freshScene } from "./seed";
import {
  current,
  currentScene,
  folderOf,
  switchConversation,
  crewLedgers,
} from "./selectors";
import { statusLabels } from "./types";
import type { Command, Ledger, Scope, TourState } from "./types";
function nextId(state: TourState, prefix: string): string {
  const ids = Object.values(state.cache).flatMap((scene) =>
    crewLedgers(scene).flatMap(([, db]) =>
      [...db.plans, ...db.requests, ...db.docs, ...db.asks].map((x) => x.id),
    ),
  );
  return (
    prefix +
    (Math.max(
      0,
      ...ids
        .filter((id) => id.startsWith(prefix))
        .map((id) => Number(id.slice(1))),
    ) +
      1)
  );
}
function showNotice(state: TourState, text: string) {
  state.notice = { id: (state.notice?.id ?? 0) + 1, text };
}
export function reducer(previous: TourState, command: Command): TourState {
  if (command.type === "restore") return command.state;
  if (
    command.type === "ui" &&
    command.patch.conv &&
    command.patch.conv !== current(previous).ui.conv
  ) {
    const next = structuredClone(previous);
    switchConversation(current(next), command.patch.conv);
    Object.assign(current(next).ui, command.patch);
    return next;
  }
  if (command.type === "ui")
    return {
      ...previous,
      cache: {
        ...previous.cache,
        [previous.sceneId]: {
          ...current(previous),
          ui: { ...current(previous).ui, ...command.patch },
        },
      },
    };
  if (command.type === "dismissNotice") return { ...previous, notice: null };
  const state = structuredClone(previous);
  try {
    if (
      "sceneId" in command &&
      command.sceneId &&
      command.sceneId !== state.sceneId
    ) {
      state.sceneId = command.sceneId;
      current(state).ui.tab = "todo";
      current(state).ui.kind = "asks";
    }
    if ("id" in command && command.id?.startsWith("H")) {
      const target = crewLedgers(current(state)).find(([, db]) =>
        db.asks.some((a) => a.id === command.id),
      );
      if (target) switchConversation(current(state), target[0]);
    }
    let { db, ui } = current(state);
    let scene = currentScene(state);
    const record = (text: string) => db.history.push({ text, time: "刚刚" });
    const message = (
      sender: string,
      text: string,
      ref: string | null = null,
      conv = ui.conv,
      internal = false,
    ) => {
      ui.counter++;
      db.messages[conv].push({
        id: "message-" + scene.id + "-" + ui.counter,
        sender,
        text,
        ref,
        internal,
        folder: db.folders.message[0],
      });
    };
    switch (command.type) {
      case "selectScene":
        if (!state.cache[command.id]) throw Error("找不到这个机组。");
        state.sceneId = command.id;
        current(state).ui.query = "";
        current(state).ui.mobileChat = false;
        break;
      case "notice":
        showNotice(state, command.text);
        break;
      case "send":
        if (!command.text.trim()) return previous;
        message("你", command.text.trim(), command.ref ?? null, ui.conv);
        ui.drafts[ui.conv] = "";
        showNotice(state, "消息已加入介绍；这里不会调用真实模型。");
        break;
      case "foldPlan":
        ui.collapsed = ui.collapsed.includes(command.id)
          ? ui.collapsed.filter((id) => id !== command.id)
          : [...ui.collapsed, command.id];
        break;
      case "shareReference":
        message("你", "分享一条关联记录：", command.id, ui.conv);
        break;
      case "answer": {
        const ask = db.asks.find((a) => a.id === command.id);
        if (!ask || !["open", "later"].includes(ask.status)) return previous;
        ask.status = "answered";
        ask.answer = command.text;
        for (const plan of db.plans.filter((p) =>
          p.blockedOn.includes(ask.id),
        )) {
          plan.blockedOn = plan.blockedOn.filter((id) => id !== ask.id);
          if (!plan.blockedOn.length) {
            plan.status = "active";
            plan.nextCheck = "现在跟进";
            plan.note = "已收到你的答复：" + command.text;
          }
        }
        message("你", command.text, ask.id);
        message(
          "机长",
          "收到。" + ask.item + " 的等待已解除，其他计划和进展也已同步。",
          ask.item,
        );
        record("你回答了 " + ask.id + "；关联计划恢复推进。");
        showNotice(state, "答复已保存；同一事项的所有引用已更新。");
        break;
      }
      case "askLater": {
        const ask = db.asks.find((a) => a.id === command.id);
        if (ask && ["open", "later"].includes(ask.status)) ask.status = "later";
        showNotice(state, "已稍后处理，相关计划继续等待。");
        break;
      }
      case "askWithdraw": {
        const ask = db.asks.find((a) => a.id === command.id);
        if (!ask || !["open", "later"].includes(ask.status)) return previous;
        ask.status = "withdrawn";
        ask.answer = "你选择不要了，相关待确认部分结束。";
        for (const p of db.plans.filter((p) => p.blockedOn.includes(ask.id))) {
          p.status = "dropped";
          p.blockedOn = [];
          p.note = "因人类撤回事项，这一部分不再继续。";
        }
        record("你撤回了 " + ask.id + "；关联部分结束。");
        break;
      }
      case "savePlan": {
        const p = db.plans.find((p) => p.id === command.id);
        if (!p) throw Error("计划不存在。");
        if (
          p.blockedOn.some((id) =>
            ["open", "later"].includes(
              db.asks.find((a) => a.id === id)?.status ?? "",
            ),
          ) &&
          command.status !== "blocked"
        )
          throw Error("这项仍在等你的答复，请先处理关联问题。");
        if (
          command.status === "done" &&
          db.plans.some(
            (c) => c.parent === p.id && !["done", "dropped"].includes(c.status),
          )
        )
          throw Error("先结束子计划，再完成这一层。");
        if (
          command.status === "active" &&
          p.dependsOn.some(
            (id) => db.plans.find((x) => x.id === id)?.status !== "done",
          )
        )
          throw Error("依赖计划还没有完成。");
        p.status = command.status;
        p.owner = command.owner;
        p.note = command.note;
        p.nextCheck = p.status === "done" ? "—" : "今天 15:00";
        if (p.status === "done")
          p.evidence = db.docs.find((d) => d.title === scene.docTitle)?.id;
        record(p.code + " 更新为" + statusLabels[p.status] + "，备注已保存。");
        showNotice(state, "已更新：Todo 和进展使用同一份计划。");
        break;
      }
      case "saveFolder": {
        const name = command.name.trim(),
          scope = command.scope;
        if (!name) throw Error("请输入文件夹名称。");
        if (
          name === "all" ||
          name === "未分组" ||
          (db.folders[scope].includes(name) && name !== command.old)
        )
          throw Error("这个名称已存在，请换一个名称。");
        if (command.old) {
          db.folders[scope] = db.folders[scope].map((f) =>
            f === command.old ? name : f,
          );
          for (const item of [
            ...db.plans,
            ...db.requests,
            ...db.asks,
            ...db.docs,
            ...Object.values(db.messages).flat(),
          ])
            if (folderOf(db, item, scope) === command.old)
              db.placements[scope + ":" + item.id] = name;
          if (ui.selectedFolders[scope] === command.old)
            ui.selectedFolders[scope] = name;
        } else db.folders[scope].push(name);
        showNotice(state, "文件夹已保存。");
        break;
      }
      case "move":
        if (![...db.folders[command.scope], "未分组"].includes(command.folder))
          throw Error("目标文件夹不存在。");
        db.placements[command.scope + ":" + command.id] = command.folder;
        showNotice(state, "已移动，原编号和引用保持不变。");
        break;
      case "newRequest": {
        if (!command.text.trim()) throw Error("先写下你的需求。");
        const id = nextId(state, "N");
        db.requests.unshift({
          id,
          code: "#" + id.slice(1),
          text: command.text.trim(),
          folder: db.folders.todo[0],
        });
        message("你", command.text, id);
        ui.tab = "todo";
        ui.kind = "requests";
        ui.selectedFolders.todo = "all";
        showNotice(state, "原文已保存，尚未生成计划。");
        break;
      }
      case "saveDataView": {
        const doc = db.docs.find((d) => d.id === command.id);
        if (!doc || !validDataView(command.dataView))
          throw Error("数据视图不存在或格式无效。");
        doc.dataView = command.dataView;
        showNotice(state, "共享内容块已保存，引用它的文档同步更新。");
        break;
      }
      case "newDocument": {
        if (!command.title.trim()) throw Error("请输入文档名称。");
        const id = nextId(state, "D");
        db.docs.unshift({
          id,
          title: command.title.trim(),
          body: command.body,
          author: "你",
          folder: db.folders.document[0],
        });
        ui.doc = id;
        break;
      }
      case "newCrew": {
        if (!command.name.trim() || !command.brief.trim())
          throw Error("请填入名称和目标。");
        if (state.cache[command.id]) throw Error("机组编号已存在。");
        const base = structuredClone(state.scenes[2]);
        Object.assign(base, {
          id: command.id,
          name: command.name,
          crew: command.name,
          project: command.name,
          request: command.brief,
          brief: command.brief,
          docTitle: command.name + "说明.md",
          docBody:
            command.brief +
            "\n\n这是在浏览器里创建的Demo 机组，没有真实调用模型。",
          note: "已保留需求，正在整理工作内容。",
        });
        const offset =
          Math.max(
            ...Object.values(state.cache).flatMap((s) =>
              crewLedgers(s).flatMap(([, db]) =>
                db.plans.map((p) => Number(p.id.slice(1))),
              ),
            ),
          ) + 100;
        const original = freshScene(state.scenes[2], true).db;
        const data = JSON.parse(
          JSON.stringify(original)
            .replace(
              /[WNHD#]\d+/g,
              (id) => id[0] + (Number(id.slice(1)) + offset),
            )
            .replaceAll("research-", command.id + "-"),
        ) as Ledger;
        data.requests[0].text = command.brief;
        data.plans[0].title = command.name;
        data.plans[0].goal = command.brief;
        const main = data.docs.find(
          (d) => d.title === state.scenes[2].docTitle,
        );
        if (main) {
          main.title = base.docTitle;
          main.body = base.docBody;
        }
        data.messages[base.id + "-main"] = [
          {
            id: base.id + "-0",
            sender: "你",
            text: command.brief,
            folder: base.folderNames.message[0],
          },
        ];
        base.initialLedger = structuredClone(data);
        state.scenes.push(base);
        state.cache[base.id] = freshScene(base, ui.inspector, data);
        state.sceneId = base.id;
        showNotice(state, "Demo 机组已创建，只保存在这个浏览器里。");
        break;
      }
      case "member":
        if (command.name === "人" || command.name === "你") {
          showNotice(state, "你是这个机组中的人类成员。");
          break;
        }
        switchConversation(
          current(state),
          scene.id + (command.name === "Todo介绍" ? "-todo" : "-agent"),
        );
        break;
      case "simulate": {
        const p = db.plans.find(
          (p) =>
            p.parent && db.plans.some((q) => q.id === p.parent && q.parent),
        );
        if (!p) throw Error("没有可推进的步骤。");
        if (p.status === "done")
          throw Error("这一步已完成，可以换个章节或重置。");
        p.status = "done";
        p.note = "该计划项已完成，产物保存在关联文档里。";
        const doc = db.docs.find((d) => d.title === scene.docTitle);
        if (doc) {
          p.evidence = doc.id;
          doc.body += "\n\n工作记录：" + p.title + "已经完成。";
        }
        record(scene.agent + "完成 " + p.code + " 并更新文档。");
        message(scene.agent, "刚完成 " + p.code + "，成果已补进文档。", p.id);
        message("系统", "完整记录：计划状态已更新。", p.id, ui.conv, true);
        showNotice(state, "计划进展已更新，计划与进展保持一致。");
        break;
      }
      case "reset":
        state.cache[scene.id] = freshScene(scene, ui.inspector);
        showNotice(state, "已恢复预置产品介绍。");
        return refreshDocumentLibrary(state);
    }
    return state;
  } catch (error) {
    return {
      ...previous,
      notice: {
        id: (previous.notice?.id ?? 0) + 1,
        text: error instanceof Error ? error.message : "无法完成这个操作。",
      },
    };
  }
}
export function scopes(): Scope[] {
  return ["message", "todo", "document", "progress"];
}
