// Agents in grouped or priority views; parents take their most urgent sub-agent's place.
// A viewed done agent stays in Done until left.
// A right-click shows the account an agent runs on and moves it to another
// one: cswap stops the agent and starts it again there, same conversation.
// The same menu restarts an agent where it is, so an update takes hold.
// Ctrl+click (Cmd on a Mac) selects several agents; a right-click on one of
// them then acts on all of them: move them to an account, restart them, or
// close them.

import { View, div, image, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { agents, terminals, panes, focused, spaces, run, theme, shared, setShared, setAgentOrder, openOverlay, closeOverlay, props, window, layout, hc, notify, exec, home, nextCommand } from "herdr";

// A sub-agent a menu asked for: started under an agent, which is told about it.
const SPAWN = "cswap.spawn";

// A switch or a restart a menu asked for. The list does the work, because it
// stays when the menu closes; whichever copy of the list sees the request
// first takes it. A restart is a job with `restart` set and no account key.
const SWITCH = "cswap.switch";

// A menu's word to the list that the selection is over.
const CLEAR = "cswap.clear";

// cswap by name, then where its installers put it: the app's PATH may lack them.
async function cswap(args, timeout = 30000) {
  let failed;
  for (const bin of ["cswap", `${home()}/.local/bin/cswap`, `${home()}/.cargo/bin/cswap`]) {
    try { return await exec(bin, args, { timeout }); } catch (e) { failed = e; }
  }
  throw failed;
}

// cswap's own words for a failure, without its name in front.
const complaint = (r) => (r.stderr || r.stdout || "cswap failed").replace(/^cswap:\s*/, "").trim();

// The sub-agents under an agent that are in a turn. They report to it by
// having herdr type into its terminal, and while it is stopped that text
// lands in a bare shell and is lost. So such an agent is asked about first,
// and in a selection it stays as it is.
const working = (info) => info?.children_working ?? [];

// Why a selected agent stays where it is, if it does.
const stays = (i) => i.error ?? i.why_not
  ?? (i.conversation === "unknown" ? "conversation not found" : null)
  ?? (working(i).length ? "sub-agents working" : null);

// The selected agents a restart can take now: on this machine, known to
// cswap, between turns, in a conversation cswap can find, and with no
// sub-agent at work.
const restartable = (infos) => (infos ?? []).filter(i => !stays(i));

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's rail goes by its width, not a flag: dragging its edge narrows it too.
const narrow = () => (find(layout(), "sidebar")?.size?.px ?? Infinity) < 160;

// To the rail (on macOS wide enough for the traffic lights), or back to the width it had.
function toggleSidebar() {
  const width = find(layout(), "sidebar")?.size?.px;
  if (!narrow()) setShared("sidebar.width", width);
  // Mods edited before the rail went by width still read the flag.
  setShared("sidebar.collapsed", !narrow());
  run("layout.resize", { id: "sidebar", px: narrow() ? (shared("sidebar.width") ?? 268) : window()?.platform === "macos" ? 76 : 44 });
}

const clean = (s) => (s ?? "").replace(/^[^\p{L}\p{N}]+/u, "").trim();

function lead(tab, all = panes()) {
  const mine = all.filter(p => p.tab === tab.id && p.terminal);
  return mine.find(p => p.program?.agent) ?? mine.find(p => p.focused) ?? mine[0];
}

function title(tab, l) {
  const prog = l?.program?.name;
  const live = clean(l?.title);
  const telling = live && live !== prog && !/^[~/]|^[A-Za-z]:\\/.test(live) && !live.includes("@") && live.toLowerCase() !== l?.program?.key;
  return (tab.name === prog || !tab.named) && telling ? live : tab.name;
}
const RANK = { blocked: 4, done: 3, working: 2, idle: 1 };
// A sub-agent without a pane isn't in agents(): its raw word, error counting as
// blocked. It's never done: it reports to its parent, and the parent's done is the news.
const kidStatus = (k, listed) => {
  const s = listed.get(k.address) ?? k.status;
  return s === "error" ? "blocked" : s === "done" ? "idle" : s;
};
const SORTS = [
  { id: "priority", label: "Priority", icon: "clock" },
  { id: "grouped", label: "Grouped by space", icon: "layers" },
];
const sortOf = () => SORTS.find((s) => s.id === shared("agents.sort")) ?? SORTS[0];
const SECTION = { 4: "Needs you", 3: "Done", 2: "Working", 1: "Idle" };
const SECTION_STATUS = { 4: "blocked", 3: "done", 2: "working" };

function duration(ms) {
  const m = Math.floor(ms / 60000);
  if (m < 1) return "<1m";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h` : `${Math.floor(h / 24)}d`;
}

// The agent's icon from the app's program table: a one-color glyph or a picture.
function icon(p, size, color) {
  if (!p?.icon) return null;
  return p.tint ? svg(p.icon).size(size).flex_none().text_color(color) : image(p.icon).w(size).h(size).flex_none();
}

export default class Agents extends View {
  init(_props, cx) {
    if (props()?.agentAddress) {
      this.menuIndex = -1;
      this.confirmIndex = 0;
      this.menuHandle = cx.focus_handle();
      this.menuHandle.focus();
      // The first menu asks cswap which account the agent runs on.
      if (!props().confirmClose && !props().accounts) props().group ? this.loadGroup(cx) : this.loadAccount(cx);
      return;
    }
    if (props()?.sortMenu) return;
    this.folded = new Set();
    // Each agent's last state and when it changed (ms), kept across restarts.
    this.changed = JSON.parse(localStorage.getItem("changed") ?? "{}");
    this.pendingPins = JSON.parse(localStorage.getItem("pins") ?? "[]");
    if (!this.pendingPins.length) localStorage.removeItem("pins");
    this.migratingPins = new Set();
    // Agents on their way to another account or being restarted, each with
    // the word its row shows meanwhile.
    this.switching = new Map();
    // Agents picked with Ctrl+click, for one action on all of them.
    this.selected = new Set();
    // The accounts view: each agent's account in place of its time and status.
    // Off at every start: it is something to glance at, not a way to work.
    this.showAccounts = false;
    this.accountOf = new Map();
    cx.timer.every(30000, (cx) => {
      if (this.showAccounts) this.loadAccounts(cx);
      this.saveAgents(cx);
      cx.notify();
    });
    this.saveAgents(cx);
    // A key can flip the view too: "<keys>": "cswap-agents.accounts" in keys.json.
    cx.spawn(async cx => {
      for (;;) {
        if ((await nextCommand()) === "accounts") this.toggleAccounts(cx);
        cx.notify();
      }
    });
  }

  // Stamps agents whose state changed. Done and idle count as one: looking at a
  // finished agent turns done into idle, and looking isn't the agent doing anything.
  track(list, now) {
    let dirty = false;
    for (const a of list) {
      const s = a.status === "done" ? "idle" : a.status;
      if (this.changed[a.address]?.s === s) continue;
      this.changed[a.address] = { s, at: now };
      dirty = true;
    }
    if (!dirty) return;
    // Gone and old: the list alone is empty right after a start.
    const listed = new Set(list.map((a) => a.address));
    for (const [k, c] of Object.entries(this.changed)) if (!listed.has(k) && now - c.at > 30 * 86400000) delete this.changed[k];
    localStorage.setItem("changed", JSON.stringify(this.changed));
  }

  // Keep offline pins until their terminal returns, and keep each pending until
  // core echoes the label so a failed write doesn't erase the old pin.
  migratePins(all) {
    const listed = new Map(all.filter((t) => t.state === "live").map((t) => [t.address, t]));
    const pending = this.pendingPins.filter((address) => {
      const terminal = listed.get(address);
      if (!terminal) return true;
      if (terminal.labels?.["herdr.pinned"]) return false;
      if (!this.migratingPins.has(address)) {
        this.migratingPins.add(address);
        run("terminal.pin", { terminal: address, pinned: true });
      }
      return true;
    });
    if (pending.length === this.pendingPins.length) return;
    this.pendingPins = pending;
    if (pending.length) localStorage.setItem("pins", JSON.stringify(pending));
    else localStorage.removeItem("pins");
  }

  since(a) {
    return this.changed[a.address]?.at ?? 0;
  }

  // Looking at a done agent makes it idle; it stays with Done until you leave
  // it rather than drop away under you. Any other change moves it at once.
  hold(list) {
    const cur = list.find((a) => a.focused);
    const same = cur && cur.address === this.held?.address;
    // Arriving, or any change but done turning idle, sets what it was.
    if (!same || cur.status !== "idle") this.held = cur ? { address: cur.address, rank: RANK[cur.status] ?? 0 } : null;
  }

  rank(a) {
    const sawDone = a.address === this.held?.address && this.held.rank === RANK.done && a.status === "idle";
    return sawDone ? RANK.done : RANK[a.status] ?? 0;
  }

  sortMenu(t) {
    const on = sortOf().id;
    return v_flex()
      .w_full()
      .p(5)
      .bg(t.popover)
      .text_size(12.8)
      .child(div().px(8).pt(4).pb(6).text_size(11).text_color(t.dim).child("Sort agents"))
      .children(SORTS.map((s) => h_flex()
        .id(`sort-${s.id}`)
        .h(31)
        .px(8)
        .gap(9)
        .rounded(7)
        .items_center()
        .text_color(t.foreground)
        .hover((el) => el.bg(t.hover))
        .on_click(() => {
          setShared("agents.sort", s.id);
          closeOverlay(props().overlay);
        })
        .child(svg(`mods/workspace-chrome/${s.icon}.svg`).size(14).flex_none().text_color(t.muted))
        .child(div().flex_1().child(s.label))
        .when(s.id === on, (el) => el.child(svg("mods/workspace-chrome/check.svg").size(14).flex_none().text_color(t.foreground)))));
  }

  // A right-click on a selected agent is about the whole selection; anywhere
  // else it is about that one agent, and the selection is dropped.
  openAgentMenu(address, cx) {
    cx.stop_propagation();
    if (this.selected.size > 1 && this.selected.has(address)) {
      openOverlay({ mod: "cswap-agents", kind: "popup", width: 264, height: 173, props: { agentAddress: address, group: [...this.selected] } });
      return;
    }
    if (this.selected.size) { this.selected.clear(); cx.notify(); }
    openOverlay({ mod: "cswap-agents", kind: "popup", width: 264, height: 177, props: { agentAddress: address } });
  }

  // Ctrl+click (Cmd on a Mac) puts an agent in the selection or takes it
  // out. A plain click drops the selection and does what it always did.
  pick(address, e, cx) {
    if (!(e?.modifiers?.control || e?.modifiers?.platform)) {
      if (this.selected.size) { this.selected.clear(); cx.notify(); }
      return false;
    }
    this.selected.has(address) ? this.selected.delete(address) : this.selected.add(address);
    cx.notify();
    return true;
  }

  // What cswap knows about each selected agent; one it cannot place says why.
  loadGroup(cx) {
    cx.spawn(async cx => {
      this.groupInfo = await Promise.all(props().group.map(async address => {
        if (address.includes("/")) return { terminal: address, error: "on another machine" };
        try {
          const r = await cswap(["herdr", "info", address, "--json"]);
          return r.code === 0 ? JSON.parse(r.stdout) : { terminal: address, error: "no cswap account" };
        } catch { return { terminal: address, error: "cswap not found" }; }
      }));
      cx.notify();
    });
  }

  // The accounts of the selection's agent, where the group menu was.
  openGroupAccountMenu(cx) {
    cx.stop_propagation();
    const infos = this.groupInfo;
    const first = infos?.find(i => !i.error);
    if (!first) return;
    const staying = infos.filter(stays).length;
    closeOverlay(props().overlay);
    openOverlay({
      mod: "cswap-agents", kind: "popup", width: 340,
      height: 44 + first.accounts.length * 31 + (staying ? 10 + staying * 19 : 0),
      props: { agentAddress: this.menuAddress(), group: props().group, accounts: first, infos },
    });
  }

  // Hand the restart of every selected agent that can restart to the list.
  requestRestartGroup(cx) {
    cx.stop_propagation();
    const addresses = restartable(this.groupInfo).map(i => i.terminal);
    closeOverlay(props().overlay);
    if (addresses.length) setShared(SWITCH, { id: Date.now(), addresses, restart: true, fresh: false, force: false });
  }

  requestCloseGroup(cx) {
    cx.stop_propagation();
    const group = props().group;
    closeOverlay(props().overlay);
    openOverlay({
      mod: "cswap-agents", kind: "dialog", width: 440, height: 220,
      props: { agentAddress: this.menuAddress(), group, confirmClose: true },
    });
  }

  groupMenu(t) {
    const group = props().group;
    const infos = this.groupInfo;
    const known = (infos ?? []).filter(i => !i.error);
    const kinds = new Set(known.map(i => i.agent));
    const names = new Set(known.map(i => i.name));
    // One list of accounts fits only agents of one kind.
    const can = known.length > 0 && kinds.size === 1;
    const account = !infos ? "\u2026"
      : !known.length ? "no cswap account"
      : kinds.size > 1 ? "different agents"
      : names.size === 1 ? [...names][0] : `${names.size} accounts`;
    const clear = () => { setShared(CLEAR, Date.now()); closeOverlay(props().overlay); };
    // Only an idle agent whose conversation cswap can find is restarted.
    const ready = restartable(infos);
    const restartNote = !infos ? "\u2026" : !ready.length ? "none can now" : ready.length < group.length ? `${ready.length} of ${group.length}` : "";
    const actions = [cx => { if (can) this.openGroupAccountMenu(cx); }, cx => { if (ready.length) this.requestRestartGroup(cx); }, cx => this.requestCloseGroup(cx), clear];
    const item = (id, label, index, note, enabled = true) => h_flex()
      .id(id).h(31).px(8).gap(8).rounded(4).items_center()
      .text_color(enabled ? t.foreground : t.dim)
      .when(enabled, el => el.cursor_pointer().hover(s => s.bg(t.hover)).on_click((_e, cx) => actions[index](cx)))
      .when(this.menuIndex === index && enabled, el => el.bg(t.hover))
      .child(div().flex_1().child(label))
      .when(Boolean(note), el => el.child(div().max_w(130).truncate().text_size(11).text_color(t.dim).child(note)));
    return v_flex()
      .id("agent-group-menu")
      .track_focus(this.menuHandle)
      .w_full().p(4).bg(t.popover).text_size(12.8)
      .on_key_down((e, cx) => {
        if (e.key === "down") this.menuIndex = (this.menuIndex + 1) % 4;
        else if (e.key === "up") this.menuIndex = this.menuIndex <= 0 ? 3 : this.menuIndex - 1;
        else if (e.key === "enter" || e.key === "space") actions[this.menuIndex]?.(cx);
        else if (e.key === "escape") closeOverlay(props().overlay);
        else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(div().h(28).px(8).flex().items_center().text_size(11).text_color(t.dim).child(`${group.length} agents selected`))
      .child(item("group-switch-account", "Switch account", 0, account, can))
      .child(item("group-restart", `Restart ${group.length} agents`, 1, restartNote, ready.length > 0))
      .child(item("group-close", `Close ${group.length} agents\u2026`, 2, ""))
      .child(div().mx(4).my(4).h(1).bg(t.border))
      .child(item("group-clear", "Clear selection", 3, ""));
  }

  toggleAccounts(cx) {
    this.showAccounts = !this.showAccounts;
    if (this.showAccounts) this.loadAccounts(cx);
    cx.notify();
  }

  // Write down which agent runs where, so `cswap herdr restore` can bring
  // them back after herdr was stopped. `--auto` leaves the file alone when
  // it is from before a stop. A cswap too old to know `save` just fails.
  saveAgents(cx) {
    cx.spawn(async () => {
      try { await cswap(["herdr", "save", "--auto", "--json"], 15000); } catch {}
    });
  }

  // Which account every agent runs on, from `cswap herdr ls`. It reads this
  // machine's processes and what cswap has stored; nothing is fetched.
  loadAccounts(cx) {
    cx.spawn(async cx => {
      try {
        const r = await cswap(["herdr", "ls", "--json"]);
        if (r.code === 0) this.accountOf = new Map(JSON.parse(r.stdout).map(a => [a.terminal, a.name]));
        this.accountsError = r.code === 0 ? null : complaint(r);
      } catch { this.accountsError = "cswap not found"; }
      cx.notify();
    });
  }

  // An agent's account, where the time and the status are otherwise.
  accountLabel(t, address) {
    const name = this.accountOf.get(address);
    return div().flex_none().max_w(110).truncate().text_size(11).text_color(name ? t.muted : t.dim).child(name ?? "\u2013");
  }

  // The + menu's choices, started under this agent instead of in a new tab.
  requestSubAgent(cx) {
    cx.stop_propagation();
    const address = this.menuAddress();
    closeOverlay(props().overlay);
    openOverlay({ mod: "cswap-new-menu", kind: "popup", width: 300, height: 512, props: { parent: address } });
  }

  // The list's side of a new sub-agent: start it under its parent, show it,
  // and tell the parent, which is how an agent learns about a helper it did
  // not start itself.
  takeSpawn(cx) {
    const job = shared(SPAWN);
    if (!job?.parent) return;
    setShared(SPAWN, null);
    if (Date.now() - job.id > 10000) return;
    cx.spawn(async cx => {
      try {
        const cwd = terminals().find(k => k.address === job.parent)?.cwd ?? home();
        let argv = job.argv ?? [];
        let keep = [];
        if (argv.length) {
          // As the + menu starts an agent: in the person's shell, which stays when the agent ends.
          const shell = (await exec("sh", ["-c", 'printf %s "$SHELL"'])).stdout.trim();
          if (/\/(zsh|bash)$/.test(shell)) argv = [shell, "-ic", '"$@"; exec "$0" -i', shell, ...argv];
          else keep = ["--keep"];
        }
        const r = await hc(["run", "--bg", "--json", "--parent", job.parent, "--cwd", cwd, ...keep, ...(argv.length ? ["--", ...argv] : [])]);
        if (r.code !== 0) throw new Error(r.stderr || "hc run failed");
        const child = JSON.parse(r.stdout).address;
        run("terminal.peek", { terminal: child });
        const note = `[herdr] The user started a sub-agent under you: ${job.label} in terminal ${child}. It has no task yet. Give it one with hc send ${child} "<task>", read it with hc read ${child}, wait for it with hc wait ${child}.`;
        const told = await hc(["send", job.parent, note]);
        if (told.code !== 0) notify({ title: `${job.parent} was not told about ${child}`, body: told.stderr.trim(), level: "warning" });
      } catch (error) {
        notify({ title: `Could not start ${job.label} under ${job.parent}`, body: String(error?.message ?? error), level: "error" });
      }
      cx.notify();
    });
  }

  // The account in use and the ones on offer, from `cswap herdr info`.
  loadAccount(cx) {
    const address = this.menuAddress();
    // cswap sees this machine's agents only.
    if (address.includes("/")) { this.accountError = "other machine"; return; }
    cx.spawn(async cx => {
      try {
        const r = await cswap(["herdr", "info", address, "--json"]);
        if (r.code === 0) this.account = JSON.parse(r.stdout);
        else this.accountError = "no cswap account";
      } catch { this.accountError = "cswap not found"; }
      cx.notify();
    });
  }

  // The list of accounts, as a second menu where the first one was.
  openAccountMenu(cx) {
    cx.stop_propagation();
    const info = this.account;
    if (!info) return;
    closeOverlay(props().overlay);
    openOverlay({
      mod: "cswap-agents", kind: "popup", width: 320,
      height: 44 + info.accounts.length * 31 + (info.why_not ? 46 : 0),
      props: { agentAddress: this.menuAddress(), accounts: info },
    });
  }

  // Hand the switch to the list.
  requestSwitch(acc, cx) {
    cx.stop_propagation();
    const info = props().accounts;
    const address = this.menuAddress();
    closeOverlay(props().overlay);
    if (props().infos) {
      // Only the agents that can move and are not there already.
      const addresses = props().infos
        .filter(i => !stays(i) && i.account !== acc.key)
        .map(i => i.terminal);
      if (addresses.length) setShared(SWITCH, { id: Date.now(), addresses, key: acc.key, name: acc.name, fresh: false, force: false });
      return;
    }
    this.askOrGo(info, { addresses: [address], key: acc.key, name: acc.name });
  }

  // Hand a switch or a restart of one agent to the list, after asking when
  // it would cost something: a conversation cswap cannot find is not carried
  // over, and sub-agents at work can lose a report.
  askOrGo(info, job) {
    const fresh = info.conversation === "unknown";
    const force = working(info).length > 0;
    if (!fresh && !force) {
      setShared(SWITCH, { id: Date.now(), ...job, fresh: false, force: false });
      return;
    }
    openOverlay({
      mod: "cswap-agents", kind: "dialog", width: 460, height: fresh && force ? 290 : force ? 230 : 210,
      props: { agentAddress: info.terminal, accounts: info, confirmSwitch: { ...job, fresh, force } },
    });
  }

  // Hand the restart to the list.
  requestRestart(cx) {
    cx.stop_propagation();
    const info = this.account;
    if (!info || info.why_not) return;
    const address = this.menuAddress();
    closeOverlay(props().overlay);
    this.askOrGo(info, { addresses: [address], restart: true, name: info.name });
  }

  accountMenu(t) {
    const info = props().accounts;
    const rows = info.accounts;
    // For a selection: who is on each account, and who cannot move at all.
    const infos = props().infos;
    const here = (acc) => (infos ?? []).filter(i => !i.error && i.account === acc.key).length;
    const all = (acc) => infos ? here(acc) === infos.length : acc.current;
    const can = (acc) => infos
      ? acc.signed_in && infos.some(i => !stays(i) && i.account !== acc.key)
      : !info.why_not && !acc.current && acc.signed_in;
    const note = (acc) => !acc.signed_in ? "not signed in"
      : !infos ? (acc.current ? "in use" : acc.usage)
      : here(acc) ? `${here(acc)} of ${infos.length} here` : acc.usage;
    const item = (acc, index) => h_flex()
      .id(`account-${acc.key}`)
      .h(31).px(8).gap(8).rounded(4).items_center()
      .text_color(can(acc) || all(acc) ? t.foreground : t.dim)
      .when(can(acc), el => el.cursor_pointer().hover(s => s.bg(t.hover)).on_click((_e, cx) => this.requestSwitch(acc, cx)))
      .when(this.menuIndex === index && can(acc), el => el.bg(t.hover))
      .child(h_flex().w(14).flex_none().items_center().justify_center()
        .when(all(acc), el => el.child(svg("mods/workspace-chrome/check.svg").size(13).text_color(t.foreground))))
      .child(div().flex_none().max_w(130).truncate().when(all(acc), el => el.font_semibold()).child(acc.name))
      .child(div().flex_1().min_w(0).truncate().text_right().text_size(11).text_color(t.dim).child(note(acc)));
    return v_flex()
      .id("agent-account-menu")
      .track_focus(this.menuHandle)
      .w_full().p(4).bg(t.popover).text_size(12.8)
      .on_key_down((e, cx) => {
        if (e.key === "down") this.menuIndex = (this.menuIndex + 1) % rows.length;
        else if (e.key === "up") this.menuIndex = this.menuIndex <= 0 ? rows.length - 1 : this.menuIndex - 1;
        else if (e.key === "enter" || e.key === "space") {
          const acc = rows[this.menuIndex];
          if (acc && can(acc)) this.requestSwitch(acc, cx);
        }
        else if (e.key === "escape") closeOverlay(props().overlay);
        else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(h_flex().h(28).px(8).gap(8).items_center().text_size(11).text_color(t.dim)
        .child(div().flex_1().truncate().child(`${info.agent_name} account`))
        .child(div().flex_none().child(infos ? `${infos.length} agents` : info.terminal)))
      .children(rows.map(item))
      .when(Boolean(infos), el => el.child(v_flex().pt(6).pb(4)
        .children(infos.filter(stays).map(i => div().px(8).h(19).truncate().text_size(11.5).text_color(t.muted).child(`${i.terminal} stays: ${stays(i)}`)))))
      .when(!infos && Boolean(info.why_not), el => el.child(div().px(8).pt(6).pb(4).text_size(11.5).text_color(t.muted).child(info.why_not)));
  }

  switchConfirmation(t) {
    const info = props().accounts;
    const to = props().confirmSwitch;
    const cancel = () => closeOverlay(props().overlay);
    const go = () => {
      setShared(SWITCH, { ...to, id: Date.now() });
      closeOverlay(props().overlay);
    };
    const kids = working(info);
    const title = to.force
      ? (to.restart ? `Restart ${info.terminal} while its sub-agents work?` : `Move ${info.terminal} while its sub-agents work?`)
      : (to.restart ? `Restart ${info.terminal} with a new conversation?` : `Start a new conversation on ${to.name}?`);
    const reasons = [];
    if (to.force) reasons.push(`${kids.length === 1 ? "A sub-agent is" : `${kids.length} sub-agents are`} still working under it: ${kids.join(", ")}. They report to ${info.terminal} when they are done, and a report that arrives while it is stopped is typed into its shell and lost. Wait for them, or go ahead and check on them afterwards.`);
    if (to.fresh) reasons.push(`cswap cannot find the conversation ${info.terminal} is in, so it cannot carry it over. ${info.agent_name} would ${to.restart ? "start again" : "start"} on ${to.name} with a new one; the old one stays in its history.`);
    const yes = to.force
      ? (to.restart ? "Restart anyway" : "Switch anyway")
      : (to.restart ? "Restart, new conversation" : "Switch, new conversation");
    const button = (id, label, index, action) => h_flex()
      .id(id).h(32).px(14).items_center().rounded(5)
      .border_1().border_color(this.confirmIndex === index ? t.foreground : t.input)
      .bg(t.popover).text_color(t.foreground).cursor_pointer()
      .hover(s => s.bg(t.hover))
      .on_click(() => action())
      .child(label);
    return v_flex()
      .id("switch-account-confirmation")
      .track_focus(this.menuHandle)
      .size_full().min_h(0).p(18).gap(12)
      .bg(t.popover).text_size(13)
      .on_key_down((e, cx) => {
        if (e.key === "escape") cancel();
        else if (e.key === "tab" || e.key === "left" || e.key === "right") this.confirmIndex = 1 - this.confirmIndex;
        else if (e.key === "enter" || e.key === "space") {
          if (this.confirmIndex === 1) go();
          else cancel();
        } else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(div().font_semibold().text_size(15).text_color(t.foreground).child(title))
      .children(reasons.map(text => div().text_color(t.muted).child(text)))
      .child(div().flex_1())
      .child(h_flex().justify_end().gap(8)
        .child(button("cancel-switch-account", "Cancel", 0, cancel))
        .child(button("confirm-switch-account", yes, 1, go)));
  }

  // Move or restart one agent and say how it went.
  async move(address, job) {
    const failed = job.restart ? `${address} was not restarted` : `${address} was not moved`;
    try {
      const what = job.restart ? ["restart", address] : ["switch", address, job.key];
      const args = ["herdr", ...what, "--json", ...(job.fresh ? ["--fresh"] : []), ...(job.force ? ["--force"] : [])];
      // A send waits for a draft someone left in the terminal, so give it time.
      const r = await cswap(args, 120000);
      if (r.code !== 0) return { ok: false, title: failed, body: complaint(r), level: "error" };
      const done = JSON.parse(r.stdout);
      const on = job.restart ? done.account : done.to;
      const talk = done.resumed ? "Same conversation." : "New conversation.";
      if (done.started) return { ok: true, title: job.restart ? `${address} restarted on ${on}` : `${address} is on ${on}`, body: talk, level: "success" };
      return { ok: false, title: `${address}: started on ${on}?`, body: "The launch was typed, but the agent is not seen running yet. Look at the terminal.", level: "warning" };
    } catch (error) {
      return { ok: false, title: failed, body: String(error), level: "error" };
    }
  }

  // The list's side of a switch or a restart: run cswap and say how it went.
  // A selection goes one agent after the other, so no two are stopped at the
  // same time.
  takeSwitch(cx) {
    const job = shared(SWITCH);
    if (!job?.addresses?.length) return;
    setShared(SWITCH, null);
    // A request nobody took in time is dropped, never run late.
    if (Date.now() - job.id > 10000) return;
    const many = job.addresses.length > 1;
    if (many) this.selected.clear();
    for (const address of job.addresses) this.switching.set(address, job.restart ? "restarting\u2026" : "switching\u2026");
    const start = job.restart
      ? (many ? `Restarting ${job.addresses.length} agents\u2026` : `Restarting ${job.addresses[0]}\u2026`)
      : (many ? `Moving ${job.addresses.length} agents to ${job.name}\u2026` : `Moving ${job.addresses[0]} to ${job.name}\u2026`);
    notify({ title: start, level: "info", timeout: 5000 });
    cx.spawn(async cx => {
      let moved = 0;
      for (const address of job.addresses) {
        const how = await this.move(address, job);
        if (how.ok) moved += 1;
        // One line each for a single agent or a failure; a group gets one summary.
        if (!many || !how.ok) notify({ title: how.title, body: how.body, level: how.level });
        this.switching.delete(address);
        cx.notify();
      }
      const sum = job.restart ? `${moved} of ${job.addresses.length} agents restarted` : `${moved} of ${job.addresses.length} agents are on ${job.name}`;
      if (many) notify({ title: sum, body: "Each kept its conversation.", level: moved === job.addresses.length ? "success" : "warning" });
      if (this.showAccounts) this.loadAccounts(cx);
      cx.notify();
    });
  }

  menuAddress() {
    const address = props()?.agentAddress;
    if (typeof address !== "string" || !address.trim()) throw new Error("Missing agent address");
    return address;
  }

  copyAgentId(cx) {
    cx.stop_propagation();
    cx.write_to_clipboard(this.menuAddress());
    closeOverlay(props().overlay);
  }

  // Include kept tabs and walk through finished ancestors. Herdr's normal
  // terminal.kill deliberately spares kept descendants, so target each one.
  closingTree(address, all) {
    const byAddress = new Map(all.map(t => [t.address, t]));
    if (byAddress.get(address)?.state !== "live") throw new Error("Agent is no longer running");
    const children = new Map();
    for (const terminal of all) {
      if (!terminal.parent) continue;
      if (!children.has(terminal.parent)) children.set(terminal.parent, []);
      children.get(terminal.parent).push(terminal.address);
    }
    const visiting = new Set();
    const visited = new Set();
    const ordered = [];
    const visit = current => {
      if (visiting.has(current)) throw new Error("Cycle in agent hierarchy");
      if (visited.has(current)) return;
      visiting.add(current);
      for (const child of children.get(current) ?? []) visit(child);
      visiting.delete(current);
      visited.add(current);
      if (byAddress.get(current)?.state === "live") ordered.push(current);
    };
    visit(address);
    return ordered;
  }

  requestCloseAgentTree(cx) {
    cx.stop_propagation();
    const address = this.menuAddress();
    closeOverlay(props().overlay);
    openOverlay({
      mod: "cswap-agents", kind: "dialog", width: 440, height: 200,
      props: { agentAddress: address, confirmClose: true },
    });
  }

  closeConfirmation(t) {
    const cancel = () => closeOverlay(props().overlay);
    const button = (id, label, index, action) => h_flex()
      .id(id).h(32).px(14).items_center().rounded(5)
      .border_1().border_color(this.confirmIndex === index ? t.foreground : t.input)
      .bg(t.popover).text_color(t.foreground).cursor_pointer()
      .hover(s => s.bg(t.hover))
      .on_click((_e, cx) => action(cx))
      .child(label);
    return v_flex()
      .id("close-agent-confirmation")
      .track_focus(this.menuHandle)
      .size_full().min_h(0).p(18).gap(12)
      .bg(t.popover).text_size(13)
      .on_key_down((e, cx) => {
        if (e.key === "escape") cancel();
        else if (e.key === "tab" || e.key === "left" || e.key === "right") this.confirmIndex = 1 - this.confirmIndex;
        else if (e.key === "enter" || e.key === "space") {
          if (this.confirmIndex === 1) this.closeAgentTree(cx);
          else cancel();
        } else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(div().font_semibold().text_size(15).text_color(t.foreground).child(props().group ? `Close ${props().group.length} agents?` : "Close agent tree?"))
      .child(div().text_color(t.muted).child(props().group
        ? `Close ${props().group.join(", ")} and all child agents beneath them, including agents in separate tabs?`
        : `Close ${this.menuAddress()} and all child agents beneath it, including agents in separate tabs?`))
      .child(div().flex_1())
      .child(h_flex().justify_end().gap(8)
        .child(button("cancel-close-agent", "Cancel", 0, cancel))
        .child(button("confirm-close-agent", this.closing ? "Closing…" : props().group ? "Close them" : "Close tree", 1, cx => this.closeAgentTree(cx))));
  }

  closeAgentTree(cx) {
    cx.stop_propagation();
    if (!props()?.confirmClose) throw new Error("Closing an agent tree requires confirmation");
    if (this.closing) return;
    const address = this.menuAddress();
    this.closing = true;
    cx.notify();
    cx.spawn(async cx => {
      try {
        // The live sidebar omits finished intermediaries whose children may
        // still run. Include retained records before walking the hierarchy.
        const result = await hc(["ls", "--all", "--json"]);
        if (result.code !== 0) throw new Error(result.stderr || "Could not load agent hierarchy");
        const records = JSON.parse(result.stdout).terminals;
        if (!Array.isArray(records)) throw new Error("Invalid agent hierarchy response");
        // A selection: every tree, each terminal once. One that ended meanwhile is passed over.
        const live = new Set(records.filter(r => r.state === "live").map(r => r.address));
        const roots = props().group ? props().group.filter(a => live.has(a)) : [address];
        const addresses = [...new Set(roots.flatMap(root => this.closingTree(root, records)))];
        // Native commands queue the shutdown work independently of this popup.
        for (const terminal of addresses) run("terminal.kill", { terminal });
        if (props().group) setShared(CLEAR, Date.now());
        closeOverlay(props().overlay);
      } catch (error) {
        this.closing = false;
        notify({ title: "Could not close agent tree", body: String(error), level: "error" });
        cx.notify();
      }
    });
  }

  agentMenu(t) {
    const item = (id, label, index, action, address, enabled = true) => h_flex()
      .id(id)
      .h(31)
      .px(8)
      .gap(8)
      .rounded(4)
      .items_center()
      .text_color(enabled ? t.foreground : t.dim)
      .when(enabled, el => el.cursor_pointer().hover(s => s.bg(t.hover)).on_click((_e, cx) => action(cx)))
      .when(this.menuIndex === index && enabled, el => el.bg(t.hover))
      .child(div().flex_1().child(label))
      .when(Boolean(address), el => el.child(div().max_w(120).truncate().text_size(11).text_color(t.dim).child(address)));
    // A sub-agent is started through this machine's hc.
    const local = !this.menuAddress().includes("/");
    // The account in use sits beside the item that changes it.
    const account = this.account ? this.account.name : this.accountError ?? "\u2026";
    // Stop it and start it again where it is: how an update takes hold.
    const canRestart = Boolean(this.account) && !this.account.why_not;
    const below = working(this.account).length;
    const restartNote = !this.account ? (this.accountError ?? "\u2026") : this.account.why_not ? "in a turn" : below ? `${below} working below` : "";
    return v_flex()
      .id("agent-context-menu")
      .track_focus(this.menuHandle)
      .w_full()
      .p(4)
      .bg(t.popover)
      .text_size(12.8)
      .on_key_down((e, cx) => {
        if (e.key === "down") this.menuIndex = (this.menuIndex + 1) % 5;
        else if (e.key === "up") this.menuIndex = this.menuIndex <= 0 ? 4 : this.menuIndex - 1;
        else if (e.key === "enter" || e.key === "space") {
          if (this.menuIndex === 4) this.requestCloseAgentTree(cx);
          else if (this.menuIndex === 3) this.requestRestart(cx);
          else if (this.menuIndex === 2) { if (local) this.requestSubAgent(cx); }
          else if (this.menuIndex === 1) this.openAccountMenu(cx);
          else this.copyAgentId(cx);
        }
        else if (e.key === "escape") closeOverlay(props().overlay);
        else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(item("copy-agent-id", "Copy agent ID", 0, cx => this.copyAgentId(cx), this.menuAddress()))
      .child(item("switch-account", "Switch account", 1, cx => this.openAccountMenu(cx), account, Boolean(this.account)))
      .child(item("add-sub-agent", "Add sub-agent\u2026", 2, cx => this.requestSubAgent(cx), local ? "" : "other machine", local))
      .child(div().mx(4).my(4).h(1).bg(t.border))
      .child(item("restart-agent", "Restart agent", 3, cx => this.requestRestart(cx), restartNote, canRestart))
      .child(item("close-agent-tree", "Close agent tree…", 4, cx => this.requestCloseAgentTree(cx)));
  }

  peek(addr) {
    run("terminal.peek", { terminal: addr });
  }

  // Live sub-agents of `addr`, depth first, each with the tree lines in front of it.
  tree(all, addr, lead = []) {
    const kids = all.filter((k) => k.parent === addr && k.state === "live");
    return kids.flatMap((k, i) => {
      const last = i === kids.length - 1;
      return [
        { t: k, lines: [...lead, last ? "└" : "├"] },
        ...this.tree(all, k.address, [...lead, last ? "" : "│"]),
      ];
    });
  }

  subRow(t, s, shown, compact) {
    const k = s.t;
    const st = kidStatus(k, this.listed);
    const pic = icon(k.program, 14, t.foreground);
    const on = k.address === shown;
    const picked = this.selected.has(k.address);
    return h_flex()
      .id(`sub-${k.address}`)
      .pl(39)
      .pr(8)
      .rounded(8)
      .h(26)
      .flex_none()
      .gap(0)
      .items_center()
      .text_size(12.5)
      .when(on, (el) => el.bg(t.raised))
      .when(picked, (el) => el.bg(t.selected))
      .hover((s) => s.bg(t.hover))
      .on_click((e, cx) => { if (!this.pick(k.address, e, cx)) this.peek(k.address); })
      .on_mouse_down("right", (_e, cx) => this.openAgentMenu(k.address, cx))
      .children(s.lines.map(l => div().self_start().mt(-4).w(9).h(17).mr(4).flex_none()
        .when(Boolean(l), el => el.border_l(1.5).border_color(t.input))
        .when(l === "└" || l === "├", el => el.border_b(1.5).rounded_bl(6))))
      .when(Boolean(pic), el => el.child(div().mr(8).child(pic)))
      .child(div().flex_1().truncate().text_color(on ? t.foreground : t.muted).child(k.name || k.template || k.program?.name || k.address))
      .when(picked, el => el.child(svg("mods/workspace-chrome/check.svg").size(13).mr(4).flex_none().text_color(t.foreground)))
      .child(this.showAccounts ? this.accountLabel(t, k.address) : this.status(t, { status: st }, compact));
  }

  // compact: a ! for blocked where a header already says "Needs you".
  status(t, a, compact = false) {
    const color = t[a.status] ?? t.dim;
    if (a.status === "working") return svg("mods/workspace-chrome/spinner.svg").size(12).mx(1).flex_none().text_color(t.working).spin(900);
    if (a.status === "blocked" && compact) return div().size(14).flex_none().rounded(4).bg(t.blocked + "26").text_size(10.5).font_bold().text_color(t.blocked).flex().items_center().justify_center().child("!");
    if (a.status === "blocked") return div().flex_none().px(6).h(18).rounded(5).bg(t.blocked + "26").text_size(10.5).font_semibold().text_color(t.blocked).flex().items_center().child("Needs you");
    // Idle: a ring; done: a dot.
    if (a.status === "idle") return div().size(7).mx(3).flex_none().rounded(4).border(1.5).border_color(t.idle ?? t.dim);
    return div().size(7).mx(3).flex_none().rounded(4).bg(a.status === "done" ? color : t.dim + "00");
  }

  render(cx) {
    const t = theme();
    if (props()?.agentAddress) {
      if (props().confirmClose) return this.closeConfirmation(t);
      if (props().confirmSwitch) return this.switchConfirmation(t);
      if (props().accounts) return this.accountMenu(t);
      return props().group ? this.groupMenu(t) : this.agentMenu(t);
    }
    if (props()?.sortMenu) return this.sortMenu(t);
    this.takeSwitch(cx);
    this.takeSpawn(cx);
    if (shared(CLEAR)) { setShared(CLEAR, null); this.selected.clear(); }
    const collapsed = narrow();
    const sort = sortOf();
    let list = agents();
    this.listed = new Map(list.map((a) => [a.address, a.status]));
    const all = terminals();
    // An agent that ended is no longer selected.
    for (const address of this.selected) if (!all.some(k => k.address === address && k.state === "live")) this.selected.delete(address);
    this.migratePins(all);
    const shown = focused()?.agent;
    const now = Date.now();
    this.track(list, now);
    this.hold(list);
    const byPriority = sort.id === "priority";
    const urgency = new Map(list.map((a) => [a.address, Math.max(this.rank(a), ...this.tree(all, a.address).map((s) => RANK[kidStatus(s.t, this.listed)] ?? 0))]));
    const rankOf = (a) => urgency.get(a.address);
    if (byPriority) list = [...list].sort((x, y) => rankOf(y) - rankOf(x) || this.since(y) - this.since(x) || y.seq - x.seq);
    const pinned = list.filter((a) => a.pinned);
    const rest = list.filter((a) => !a.pinned);
    list = [...pinned, ...rest];
    setAgentOrder(list.map((a) => a.address));

    const withKids = (a, i) => {
      const subs = collapsed ? [] : this.tree(all, a.address);
      const r = row(a, i, subs);
      if (this.folded.has(a.address)) return [r];
      return [r, ...subs.map((s) => this.subRow(t, s, shown, byPriority))];
    };

    // The fold chevron; folded, the count too, colored by the most urgent one.
    const fold = (a, subs) => {
      const sts = subs.map((s) => kidStatus(s.t, this.listed));
      const top = sts.reduce((m, s) => ((RANK[s] ?? 0) > (RANK[m] ?? 0) ? s : m), null);
      const open = !this.folded.has(a.address);
      return div()
        .id(`fold-${a.address}`)
        .h(18)
        .px(3)
        .gap(3)
        .flex()
        .items_center()
        .rounded(4)
        .text_size(11)
        .text_color(open ? t.dim : (t[top] ?? t.dim))
        .cursor_pointer()
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .on_click((_e, cx) => {
          cx.stop_propagation();
          open ? this.folded.add(a.address) : this.folded.delete(a.address);
          cx.notify();
        })
        .child(svg(`mods/workspace-chrome/chevron-${open ? "down" : "right"}.svg`).size(11).flex_none())
        .when(!open, (el) => el.child(String(subs.length)));
    };

    // A row reads "space \u00b7 detail": the detail is a tab someone named or the
    // agent's own title, never an automatic tab number unless two rows would
    // otherwise read the same.
    const tabs = spaces().flatMap(s => s.tabs);
    const label = (a) => {
      const tab = tabs.find(tab => tab.id === a.tab);
      return tab ? title(tab, lead(tab)) : a.title || a.program?.name || "";
    };
    const detail = new Map(list.map(a => [a.address, /^\d+$/.test(label(a)) ? null : label(a)]));
    const seen = {};
    for (const a of list) { const k = `${a.space_name}\u0000${detail.get(a.address) ?? ""}`; seen[k] = (seen[k] ?? 0) + 1; }
    for (const a of list) {
      if (detail.get(a.address) == null && seen[`${a.space_name}\u0000`] > 1) detail.set(a.address, label(a));
    }

    const row = (a, i, subs = []) => {
      const pic = icon(a.program, 16, t.foreground);
      const more = detail.get(a.address);
      // An agent on another machine (`vps/t7`) says which.
      const machine = a.address.includes("/") ? a.address.split("/")[0] : null;
      if (collapsed) {
        return h_flex()
          .id(`ag-${a.address}`)
          .h(26)
          .px(8)
          .flex_none()
          .items_center()
          .gap(4)
          .hover((s) => s.bg(t.hover))
          .on_click(() => run("pane.focus", { pane: a.pane }))
          .on_mouse_down("right", (_e, cx) => this.openAgentMenu(a.address, cx))
          .child(div().flex_1().min_w(0).whitespace_nowrap().text_right().text_color(t.muted).child(String(i + 1)))
          .child(h_flex().w(14).flex_none().items_center().justify_center().child(this.status(t, a, true)));
      }
      const when = this.since(a);
      const idle = a.status === "idle" && !a.focused;
      const isPinned = a.pinned;
      const picked = this.selected.has(a.address);
      const pin = div()
        .id(`pin-${a.address}`)
        .flex_none()
        .p(2)
        .rounded(4)
        .text_color(t.dim)
        .cursor_pointer()
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .tooltip(isPinned ? "Unpin" : "Pin to the top")
        .on_click((_e, cx) => {
          cx.stop_propagation();
          run("terminal.pin", { terminal: a.address, pinned: !isPinned });
          // The row moves away from under the pointer, and no hover-out comes.
          this.hovered = null;
          cx.notify();
        })
        .child(svg(`mods/workspace-chrome/${isPinned ? "pin-off" : "pin"}.svg`).size(13));
      // Under a space header the space goes without saying; an automatic tab number says nothing.
      // Without a space header above (priority, pinned) the row names its space.
      const full = byPriority || isPinned;
      const name = full ? a.space_name || label(a) : more ?? (/^\d+$/.test(label(a)) ? a.program?.name || label(a) : label(a));
      return h_flex()
        .id(`ag-${a.address}`)
        .px(8)
        .h(30)
        .flex_none()
        .rounded(8)
        .items_center()
        .gap(9)
        .when(a.focused || picked, (el) => el.bg(t.selected))
        .cursor_pointer()
        .hover((s) => s.bg(t.hover))
        .on_hover((on, cx) => {
          if (on) this.hovered = a.address;
          else if (this.hovered === a.address) this.hovered = null;
          cx.notify();
        })
        .on_click((e, cx) => { if (!this.pick(a.address, e, cx)) run("pane.focus", { pane: a.pane }); })
        .on_mouse_down("right", (_e, cx) => this.openAgentMenu(a.address, cx))
        .when(Boolean(pic), el => el.child(div().id(`agent-fold-${a.address}`)
          .when(subs.length > 0, el => el.on_click((_e, cx) => {
            cx.stop_propagation();
            this.folded.has(a.address) ? this.folded.delete(a.address) : this.folded.add(a.address);
            cx.notify();
          })).child(pic)))
        .child(h_flex().flex_1().min_w(0).gap(5).items_center()
          .child(div().min_w(0).max_w(full && more ? 120 : 200).truncate().text_color(idle ? t.muted : t.foreground).child(name))
          .when(full && Boolean(more), el => el.child(div().flex_1().min_w(0).truncate().text_color(t.dim).child(`\u00b7 ${more}`))))
        .when(Boolean(machine), el => el.child(div().text_size(10).text_color(t.dim).child(machine)))
        .when(subs.length > 0, el => el.child(fold(a, subs)))
        .when(this.switching.has(a.address), el => el.child(div().flex_none().text_size(11).text_color(t.dim).child(this.switching.get(a.address))))
        .when(this.showAccounts && !this.switching.has(a.address), el => el.child(this.accountLabel(t, a.address)))
        .when(!this.showAccounts && !picked && this.hovered === a.address, el => el.child(pin))
        .when(!this.showAccounts && !picked && this.hovered !== a.address && !this.switching.has(a.address) && Boolean(when), el => el.child(div().flex_none().text_size(11).text_color(t.dim).child(duration(now - when))))
        .when(!this.showAccounts && !picked, el => el.child(this.status(t, a, byPriority && !isPinned)))
        // A selected agent wears a check where its time and status were.
        .when(picked, el => el.child(svg("mods/workspace-chrome/check.svg").size(13).mx(1).flex_none().text_color(t.foreground)));
    };

    // Groups in list order: spaces (grouped) or priority sections. Agents of a
    // space are next to each other in agents(), so neighbours make a group.
    const groups = pinned.length ? [{ id: "pinned", label: "Pinned", status: null, members: pinned }] : [];
    for (const a of rest) {
      const r = rankOf(a) || RANK.idle;
      const id = byPriority ? `sec-${r}` : `sp-${a.space}`;
      if (groups.at(-1)?.id === id) groups.at(-1).members.push(a);
      else groups.push({ id, label: byPriority ? SECTION[r] : a.space_name || "", status: byPriority ? SECTION_STATUS[r] : null, members: [a] });
    }

    // A group's header; it folds the group. Folded, it says how many and the most urgent state.
    const header = (g) => {
      const open = !this.folded.has(g.id);
      const top = g.members.reduce((m, a) => ((RANK[a.status] ?? 0) > (RANK[m] ?? 0) ? a.status : m), null);
      return h_flex()
        .id(g.id)
        .px(8)
        .pt(8)
        .h(30)
        .flex_none()
        .gap(6)
        .items_center()
        .text_size(11)
        .font_medium()
        .cursor_pointer()
        .hover((s) => s.text_color(t.foreground))
        .on_click((_e, cx) => {
          open ? this.folded.add(g.id) : this.folded.delete(g.id);
          cx.notify();
        })
        .child(div().truncate().text_color(t[g.status] ?? t.dim).child(g.label))
        .when(byPriority || !open, (el) => el.child(div().text_color(t.dim).child(String(g.members.length))))
        .child(div().flex_1())
        .when(!open, (el) => el
          .when(!byPriority && top && top !== "idle", (el) => el.child(this.status(t, { status: top }, true)))
          .child(svg("mods/workspace-chrome/chevron-right.svg").size(11).flex_none().text_color(t.dim)));
    };
    const rows = collapsed
      ? list.flatMap(withKids)
      : groups.flatMap((g) => [header(g), ...(this.folded.has(g.id) ? [] : g.members.flatMap(withKids))]);

    return v_flex()
      .size_full()
      .min_h(0)
      .bg(t.chrome)
      .border_t(1)
      .border_color(t.border)
      .text_size(13)
      .when(!collapsed, (el) =>
        el.child(
          h_flex()
            .px(16)
            .h(41)
            .pt(14)
            .pb(5)
            .flex_none()
            .items_center()
            .justify_between()
            .child(this.selected.size
              ? div().id("ag-selected").text_size(11.5).font_medium().text_color(t.foreground).cursor_pointer()
                  .tooltip("Clear the selection")
                  .on_click((_e, cx) => { this.selected.clear(); cx.notify(); })
                  .child(`${this.selected.size} selected \u00d7`)
              : div().text_size(11.5).font_medium().text_color(t.dim).child("Agents"))
            .child(
              h_flex()
                .gap(10)
                .items_center()
                .child(
                  div()
                    .id("ag-accounts")
                    .text_size(11)
                    .text_color(this.showAccounts ? t.foreground : t.dim)
                    .hover((s) => s.text_color(t.foreground))
                    .on_click((_e, cx) => this.toggleAccounts(cx))
                    .tooltip(this.showAccounts ? "Back to time and status" : "Show each agent's account")
                    .child(svg("mods/cswap-chrome/cswap.svg").size(14)),
                )
                .child(
                  div()
                    .id("ag-sort")
                    .text_size(11)
                    .text_color(t.dim)
                    .hover((s) => s.text_color(t.foreground))
                    .on_click(() => openOverlay({ mod: "cswap-agents", kind: "popup", width: 200, height: 140, props: { sortMenu: true } }))
                    .tooltip(`Sort agents: ${sort.label}`)
                    .child(svg(`mods/workspace-chrome/${sort.icon}.svg`).size(14)),
                ),
            ),
        ),
      )
      .when(list.length === 0 && !collapsed, (el) =>
        el.child(div().px(12).text_size(12).text_color(t.dim).child("No agent status yet. Agents show up here when they report their status. Older versions and agents inside tmux may not.")),
      )
      .child(v_flex().id("ag-list").flex_1().min_h(0).px(collapsed ? 0 : 8).overflow_y_scrollbar().mode("always").children(rows))
      .when(collapsed, (el) =>
        el.child(
          h_flex()
            .h(28)
            .items_center()
            .justify_center()
            .child(
              div()
                .id("collapse")
                .text_size(13)
                .text_color(t.dim)
                .hover((s) => s.text_color(t.foreground))
                .on_click(toggleSidebar)
                .child("»"),
            ),
        ),
      );
  }
}
