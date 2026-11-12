import { View, div, image, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { theme, run, props, window, shared, setShared, updates, layout, terminals, hc, exec, home, openOverlay, closeOverlay, notify, nextCommand } from "herdr";

// The sidebar's chrome with one more row in the footer: cswap, which opens
// the cswap dashboard in a terminal over the window. Closing it ends the
// dashboard, so nothing keeps fetching usage in the background.
const DASHBOARD = "cswap-dashboard";

// cswap by name, then where its installers put it: the app's PATH may lack them.
async function cswapPath() {
  for (const bin of ["cswap", `${home()}/.local/bin/cswap`, `${home()}/.cargo/bin/cswap`]) {
    try { if ((await exec(bin, ["--version"])).code === 0) return bin; } catch { /* try the next */ }
  }
  return null;
}

// How long ago, in a few letters: "now", "4 min", "3 h", "2 d".
function ago(seconds) {
  if (seconds < 90) return "just now";
  if (seconds < 5400) return `${Math.round(seconds / 60)} min ago`;
  if (seconds < 172800) return `${Math.round(seconds / 3600)} h ago`;
  return `${Math.floor(seconds / 86400)} d ago`;
}

const count = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

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

export default class WorkspaceChrome extends View {
  // Keys can open the dashboard and the restore dialog too:
  // "<keys>": "cswap-chrome.dashboard" or "cswap-chrome.restore" in keys.json.
  init(_props, cx) {
    if (props()?.restore) { this.loadPlan(cx); return; }
    if (!props()?.footer) return;
    // Which agent runs where is written down for `cswap herdr restore`.
    // cswap is asked every minute and writes when a save is due: the time
    // between saves is its own setting.
    this.checkSave(cx, false);
    cx.timer.every(60000, (cx) => this.checkSave(cx, false));
    cx.spawn(async cx => {
      for (;;) {
        const command = await nextCommand();
        if (command === "dashboard") this.openDashboard(cx);
        else if (command === "restore") this.openRestore();
        cx.notify();
      }
    });
  }
  async cswap(args, timeout = 30000) {
    this.bin ??= await cswapPath();
    if (!this.bin) throw new Error("cswap is not installed");
    return exec(this.bin, args, { timeout });
  }
  // Ask cswap to save: when one is due, or now when the button asks.
  checkSave(cx, byHand) {
    if (this.saving) return;
    this.saving = true;
    cx.spawn(async cx => {
      try {
        const r = await this.cswap(["herdr", "save", ...(byHand ? [] : ["--auto"]), "--json"]);
        if (r.code !== 0) throw new Error(r.stderr || "cswap could not save");
        this.saved = JSON.parse(r.stdout);
        this.saveError = null;
        if (byHand) notify({ title: `Saved ${count(this.saved.agents, "agent")}`, body: "cswap herdr restore brings them back after a stop." });
      } catch (error) {
        this.saveError = String(error?.message ?? error).trim().split("\n")[0];
        if (byHand) notify({ title: "Not saved", body: this.saveError, level: "error" });
      }
      this.saving = false;
      cx.notify();
    });
  }
  openRestore() {
    openOverlay({ mod: "cswap-chrome", kind: "dialog", modal: true, width: 720, height: 520, props: { restore: true } });
  }
  // The row over the footer's line: when the agents were last written down,
  // a button to do it now, and the way to the restore dialog.
  saveRow(t, collapsed) {
    const s = this.saved;
    const pending = Boolean(s?.pending);
    // The clock moves between two answers of cswap.
    const age = s?.saved_at == null ? null : ago(Math.max(0, Math.round(Date.now() / 1000) - s.saved_at));
    const text = this.saveError ? "save: cswap failed"
      : !s ? "\u2026"
      : pending ? `herdr was stopped \u00b7 ${count(s.agents, "agent")} to restore`
      : age == null ? "nothing saved yet"
      : `saved ${age} \u00b7 ${count(s.agents, "agent")}`;
    const note = this.saveError ? this.saveError
      : pending ? "The saved list is from before herdr was stopped. Restore brings the agents back; Save writes down what runs now instead."
      : !s ? ""
      : s.every_minutes ? `cswap saves every ${s.every_minutes} min, which you can change in its settings.`
      : "Automatic saving is off in cswap's settings.";
    const button = (id, label, action) => div().id(id).flex_none().h(20).px(7).flex().items_center().rounded(5)
      .border_1().border_color(t.border).text_size(11).text_color(t.muted).cursor_pointer()
      .hover(el => el.bg(t.hover).text_color(t.foreground)).on_click(action).child(label);
    if (collapsed) {
      return div().id("cswap-saved").h(32).w(32).flex().flex_none().items_center().justify_center().rounded(8).cursor_pointer()
        .text_color(pending ? t.accent : t.dim).hover(el => el.bg(t.hover).text_color(t.foreground))
        .tooltip(`${text}. Click to restore.`).on_click(() => this.openRestore()).child("\u21ba");
    }
    return h_flex().id("cswap-saved").h(32).flex_none().px(16).gap(6).items_center().text_size(11.5)
      .child(div().id("cswap-saved-text").flex_1().min_w(0).truncate().text_color(pending ? t.accent : t.dim)
        .when(Boolean(note), el => el.tooltip(note)).child(text))
      .child(button("cswap-save-now", this.saving ? "\u2026" : "Save", (_e, cx) => this.checkSave(cx, true)))
      .child(button("cswap-restore", "Restore", () => this.openRestore()));
  }
  // ----------------------------------------------------- the restore dialog
  // What a restore would do with every saved agent, from cswap's dry run.
  loadPlan(cx) {
    this.loading = true;
    cx.spawn(async cx => {
      try {
        const r = await this.cswap(["herdr", "restore", "--dry-run", "--json", ...(this.previous ? ["--previous"] : [])]);
        if (r.code !== 0) throw new Error(r.stderr || "cswap could not read the save");
        this.plan = JSON.parse(r.stdout);
        this.planError = null;
      } catch (error) {
        this.plan = null;
        this.planError = String(error?.message ?? error).replace(/^(cswap: )?(error: )?/i, "").trim();
      }
      this.loading = false;
      cx.notify();
    });
    cx.notify();
  }
  // Restore every agent, or the one named. The answer says what became of each.
  restore(cx, only) {
    if (this.working) return;
    this.working = only ?? "all";
    cx.spawn(async cx => {
      try {
        const args = ["herdr", "restore", ...(only ? [only] : []), "--json", ...(this.previous ? ["--previous"] : [])];
        const r = await this.cswap(args, 600000);
        const done = JSON.parse(r.stdout).terminals;
        this.outcome ??= new Map();
        for (const row of done) this.outcome.set(row.terminal, row);
        const back = done.filter(row => row.did === "run" || (row.did === "typed" && row.started)).length;
        const failed = done.filter(row => row.did === "failed" || (row.did === "typed" && !row.started)).length;
        notify({
          title: failed ? `${count(failed, "agent")} did not start` : back ? `${count(back, "agent")} restored` : "Nothing to restore",
          body: failed ? "Look at the list: Copy gives the line to start one by hand." : "Each is back in its conversation, idle.",
          level: failed ? "error" : "info",
        });
      } catch (error) {
        notify({ title: "Not restored", body: String(error?.message ?? error), level: "error" });
      }
      this.working = null;
      this.loadPlan(cx);
    });
    cx.notify();
  }
  restoreDialog(t) {
    const close = () => closeOverlay(props().overlay);
    const plan = this.plan;
    const rows = plan?.terminals ?? [];
    const can = (row) => row.did === "typed" || row.did === "run";
    const todo = rows.filter(can).length;
    const button = (id, label, on, action) => div().id(id).flex_none().h(24).px(9).flex().items_center().rounded(5)
      .border_1().border_color(t.border).text_size(12)
      .text_color(on ? t.foreground : t.dim)
      .when(on, el => el.cursor_pointer().hover(s => s.bg(t.hover)).on_click(action))
      .child(label);
    const what = (row) => {
      const was = this.outcome?.get(row.terminal);
      if (this.working === row.terminal || (this.working === "all" && can(row))) return ["starting\u2026", t.muted];
      if (was?.did === "failed") return [`not started: ${was.why}`, t.accent];
      if (was?.did === "typed" && !was.started) return ["typed, but not seen running yet: look at the terminal", t.accent];
      if (was?.did === "run") return [`started again as ${was.now}`, t.muted];
      if (was?.did === "typed") return ["started again in its terminal", t.muted];
      if (row.did === "typed") return ["its terminal is back at a shell: start it there", t.foreground];
      if (row.did === "run") return ["its terminal is gone: start a new one", t.foreground];
      return [row.why, t.dim];
    };
    const item = (row) => {
      const [text, color] = what(row);
      return h_flex().id(`restore-${row.terminal}`).flex_none().px(10).py(6).gap(10).items_center().rounded(5).hover(s => s.bg(t.hover))
        .child(v_flex().flex_1().min_w(0).gap(1)
          .child(h_flex().gap(8).items_center()
            .child(div().flex_none().font_semibold().text_color(t.foreground).child(row.name ? `${row.terminal} ${row.name}` : row.terminal))
            .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(`${row.agent_name ?? row.agent} on ${row.account}${row.session ? "" : " \u00b7 new conversation"}`)))
          .child(div().truncate().text_size(11.5).text_color(color).child(text))
          .when(Boolean(row.cwd), el => el.child(div().truncate().text_size(11).text_color(t.dim).child(row.cwd))))
        .child(button(`copy-${row.terminal}`, this.copied === row.terminal ? "Copied" : "Copy command", true, (_e, cx) => {
          cx.write_to_clipboard(row.manual);
          this.copied = row.terminal;
          cx.notify();
        }))
        .child(button(`one-${row.terminal}`, "Restore", can(row) && !this.working, (_e, cx) => this.restore(cx, row.terminal)));
    };
    const head = this.planError ? this.planError
      : !plan ? "Reading the save\u2026"
      : `${this.previous ? "The save before the last" : "Saved"} ${ago(Math.max(0, plan.now - plan.saved_at))} \u00b7 ${count(rows.length, "agent")} \u00b7 ${todo} to restore`;
    return v_flex().id("cswap-restore").size_full().min_h(0).p(16).gap(10).bg(t.popover).text_size(13)
      .child(h_flex().gap(8).items_center()
        .child(svg("mods/cswap-chrome/cswap.svg").size(14).flex_none().text_color(t.foreground))
        .child(div().font_semibold().text_size(15).text_color(t.foreground).child("Restore agents"))
        .child(div().flex_1())
        .child(div().id("cswap-restore-close").px(8).py(2).rounded(5).text_color(t.muted).cursor_pointer()
          .hover(s => s.bg(t.hover).text_color(t.foreground)).on_click(close).child("\u2715")))
      .child(div().text_color(t.muted).child(head))
      .child(div().text_size(11.5).text_color(t.dim).child("Each agent comes back idle, on its account and in its conversation. Restore does one, Restore all does every one that is not running. If one does not start, Copy command gives the line to run by hand in a terminal of your own."))
      .child(v_flex().id("cswap-restore-list").flex_1().min_h(0).overflow_y_scrollbar().children(rows.map(item)))
      .child(h_flex().gap(8).items_center()
        .child(button("cswap-restore-previous", this.previous ? "Use the last save" : "Use the save before", !this.working, (_e, cx) => {
          this.previous = !this.previous;
          this.outcome = null;
          this.loadPlan(cx);
        }))
        .child(button("cswap-restore-reload", "Check again", !this.working && !this.loading, (_e, cx) => this.loadPlan(cx)))
        .child(div().flex_1())
        .child(button("cswap-restore-cancel", "Close", true, close))
        .child(button("cswap-restore-all", this.working === "all" ? "Restoring\u2026" : `Restore all (${todo})`, todo > 0 && !this.working, (_e, cx) => this.restore(cx))));
  }
  // The dashboard in a hidden terminal of its own, shown as a dialog: a bar
  // with a close button over the terminal. One at a time: a dashboard that is
  // still running is shown again instead of starting a second.
  openDashboard(cx) {
    if (this.opening) return;
    this.opening = true;
    cx.spawn(async cx => {
      try {
        let terminal = terminals().find(t => t.name === DASHBOARD && t.state === "live")?.address;
        if (!terminal) {
          const bin = await cswapPath();
          if (!bin) throw new Error("cswap is not installed");
          const r = await hc(["run", "--bg", "--json", "--no-parent", "--no-notify", "--rm", "--name", DASHBOARD, "--cwd", home(), "--", bin, "dashboard", "usage"]);
          if (r.code !== 0) throw new Error(r.stderr || "could not start cswap");
          terminal = JSON.parse(r.stdout).address;
        }
        openOverlay({
          kind: "dialog", modal: true, width: 0.86, height: 0.88,
          layout: { id: "cswap-dashboard", type: "column", children: [
            { id: "cswap-dashboard-bar", type: "block", content: { mod: "cswap-chrome", props: { dashboard: terminal } }, size: { px: 34 } },
            { id: "cswap-dashboard-terminal", type: "block", content: { terminal } },
          ] },
        });
      } catch (error) {
        notify({ title: "Could not open cswap", body: String(error?.message ?? error), level: "error" });
      }
      this.opening = false;
      cx.notify();
    });
  }
  // The bar over the dashboard. Closing ends the terminal, and the dialog with it.
  dashboardBar(t) {
    const terminal = props().dashboard;
    const close = () => { run("terminal.kill", { terminal }); closeOverlay(props().overlay); };
    return h_flex().size_full().px(12).gap(8).items_center().bg(t.surface).border_b(1).border_color(t.border).text_size(12)
      .child(svg("mods/cswap-chrome/cswap.svg").size(14).flex_none().text_color(t.foreground))
      .child(div().font_semibold().text_color(t.foreground).child("cswap"))
      .child(div().truncate().text_color(t.dim).child("accounts and usage"))
      .child(div().flex_1())
      .child(div().text_color(t.dim).child("esc steps back"))
      .child(div().id("cswap-dashboard-close").px(8).py(2).rounded(5).text_color(t.muted).cursor_pointer()
        .hover(s => s.bg(t.hover).text_color(t.foreground)).on_click(close).child("\u2715"));
  }
  button(id, label, action) {
    const t = theme();
    return div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.muted).hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).on_click(action).child(label);
  }
  render() {
    const t = theme();
    if (props()?.dashboard) return this.dashboardBar(t);
    if (props()?.restore) return this.restoreDialog(t);
    const collapsed = narrow();
    if (props()?.footer) {
      const row = (id, icon, label, dim, action) => h_flex().id(id).h(28).px(8).gap(9).items_center().rounded(8)
        .when(collapsed, el => el.relative().w(32).px(0).justify_center().tooltip(label))
        .text_color(dim ? t.dim : t.muted).hover(s => s.bg(t.hover).text_color(t.foreground)).cursor_pointer()
        .on_click(action).child(svg(icon === "cswap" ? "mods/cswap-chrome/cswap.svg" : `mods/workspace-chrome/${icon}.svg`).size(14))
        .when(!collapsed, el => el.child(label));
      const ready = div().size(6).rounded_full().bg(t.accent);
      return v_flex().size_full().bg(t.chrome).when(collapsed, el => el.items_center())
        .child(this.saveRow(t, collapsed))
        .child(v_flex().w_full().flex_1().border_t(1).border_color(t.border).px(8).pt(4).pb(8).text_size(13)
        .when(collapsed, el => el.px(0).items_center())
        .child(row("settings", "settings", "Settings", false, () => run("app.settings"))
          .when(Boolean(updates().app.ready), el => el.child(collapsed ? ready.absolute().top(5).right(6) : ready)))
        .child(row("cswap", "cswap", "cswap", false, (_e, cx) => this.openDashboard(cx)))
        .child(row("machine", "plus", "Add machine", true, () => run("mod.open", { name: "machine-add" }))));
    }
    return h_flex().size_full().bg(t.chrome).pl(collapsed ? 6 : 14).pr(10).gap(4).items_center().text_size(13.5)
      .when(window()?.platform === "macos" && !window()?.fullscreen && !collapsed, el => el.pl(76))
      .when(!collapsed, el => el.child(h_flex().flex_1().gap(8).items_center().child(image("frame/welcome/herdr.png").size(20).rounded(5)).child(div().font_semibold().text_color(t.foreground).child("Herdr"))))
      .when(!collapsed, el => el.child(this.button("search", svg("mods/workspace-chrome/search.svg").size(15), () => run("mod.open", { name: "picker" }))))
      // On the macOS rail the traffic lights fill the row; the agents list's » unfolds it.
      .when(!(collapsed && window()?.platform === "macos"), el => el.child(this.button("sidebar-toggle", svg("mods/workspace-chrome/sidebar.svg").size(15), toggleSidebar)));
  }
}
