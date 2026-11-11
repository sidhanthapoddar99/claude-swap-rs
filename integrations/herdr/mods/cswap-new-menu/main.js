import { View, div, svg, image } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { theme, spaces, panes, knownAgents, run, openOverlay, closeOverlay, props, window, keys, shared, setShared, exec, home } from "herdr";

// The + menu, with accounts: an agent that cswap holds accounts for starts
// through cswap, and its arrow lists the accounts to start it on.

// The last agents each machine reported, shown at once while knownAgents() checks again.
const KNOWN = "new-menu.agents";
// With props { parent }, the choices start under that agent as its
// sub-agents: the agents list does the starting and tells the parent.
const SPAWN = "cswap.spawn";
// What `cswap herdr menu` last said, shown at once while it is asked again.
const MENU = "cswap.menu";

// cswap by name, then where its installers put it: the app's PATH may lack them.
async function cswap(args) {
  let failed;
  for (const bin of ["cswap", `${home()}/.local/bin/cswap`, `${home()}/.cargo/bin/cswap`]) {
    try { return await exec(bin, args); } catch (e) { failed = e; }
  }
  throw failed;
}

function shortcut(name) {
  const k = keys().find(b => b.section === "global" && b.action === name);
  if (!k?.keys.trim()) return "";
  const mac = window()?.platform === "macos";
  const names = mac ? { ctrl: "⌃", alt: "⌥", shift: "⇧", cmd: "⌘", fn: "fn" } : { ctrl: "Ctrl", alt: "Alt", shift: "Shift", cmd: "Super", fn: "Fn" };
  const alias = { secondary: mac ? "cmd" : "ctrl", super: "cmd", win: "cmd" };
  return k.keys.trim().split(/\s+/).map(stroke => {
    const parts = stroke.split("-");
    let key = parts.pop();
    if (key === "") { key = "-"; parts.pop(); }
    const held = new Set(parts.map(m => alias[m.toLowerCase()] ?? m.toLowerCase()));
    if (/^[A-Z]$/.test(key)) held.add("shift");
    const label = key.length === 1 ? key.toUpperCase() : key[0].toUpperCase() + key.slice(1).toLowerCase();
    return [...Object.keys(names).filter(m => held.has(m)).map(m => names[m]), label].join(mac ? "" : "+");
  }).join(" ");
}

export default class NewMenu extends View {
  init(_props, cx) {
    const machine = spaces().find(s => s.active)?.machine;
    const where = machine ?? "";
    this.agents = shared(KNOWN)?.[where] ?? [];
    this.handle = cx.focus_handle();
    this.handle.focus();
    cx.spawn(async cx => {
      try {
        this.agents = await knownAgents({ on: machine });
        setShared(KNOWN, { ...shared(KNOWN), [where]: this.agents });
      }
      catch { this.failed = true; }
      cx.notify();
    });
    // cswap knows this machine's accounts only.
    this.menu = machine ? [] : shared(MENU) ?? [];
    this.open = null;
    if (!machine) cx.spawn(async cx => {
      try {
        const r = await cswap(["herdr", "menu"]);
        if (r.code === 0) { this.menu = JSON.parse(r.stdout); setShared(MENU, this.menu); }
      } catch { /* no cswap: the menu works as it did */ }
      cx.notify();
    });
  }
  // cswap's entry for an agent the menu can start: its accounts and its default.
  entry(a) {
    const bin = (a?.run?.[0] ?? "").split("/").pop();
    return this.menu.find(m => m.bins.includes(bin)) ?? null;
  }
  // Start an agent in a new tab, on an account when one is named.
  start(a, account) {
    const e = this.entry(a);
    const argv = e ? ["cswap", e.agent, ...(account ? [`--cswap=${account}`] : []), ...a.run.slice(1)] : a.run;
    const parent = props()?.parent;
    if (parent) setShared(SPAWN, { id: Date.now(), parent, argv, label: a.name });
    else run("tab.new", { name: a.name, space: spaces().find(s => s.active)?.id, argv });
  }
  // Where a plain start of this agent goes: the default account's name.
  startsOn(e) {
    if (!e) return "";
    return e.start === "auto" ? "auto" : e.accounts.find(x => x.key === e.start)?.name ?? "";
  }
  // The accounts of one agent, in place of the menu.
  accounts(t, a, e) {
    const pick = (account) => { closeOverlay(props().overlay); this.start(a, account); };
    const line = (id, mark, name, hint, enabled, action) => h_flex().id(id).h(31).px(9).gap(8).rounded(7).items_center().flex_none()
      .text_color(enabled ? t.foreground : t.dim).when(enabled, el => el.hover(s => s.bg(t.selected)).on_click(action))
      .child(div().w(12).flex_none().text_color(t.muted).child(mark))
      .child(div().flex_none().max_w(120).truncate().child(name))
      .child(div().flex_1().min_w(0).truncate().text_right().text_size(11).text_color(t.dim).child(hint));
    return v_flex().min_h(0).w_full().h(Math.min(491, 62 + (e.accounts.length + 1) * 31)).bg(t.popover).p(5).text_size(12.8)
      .id("new-menu-accounts").track_focus(this.handle)
      .on_key_down((ev, cx) => {
        if (ev.modifiers.control || ev.modifiers.platform || ev.modifiers.alt) return;
        if (ev.key === "left" || ev.key === "backspace") { this.open = null; cx.stop_propagation(); cx.notify(); return; }
        const acc = /^[1-9]$/.test(ev.key) ? e.accounts[Number(ev.key) - 1] : null;
        if (acc?.signed_in) pick(acc.selector);
      })
      .child(h_flex().id("accounts-back").h(31).px(9).gap(8).rounded(7).items_center().flex_none().text_color(t.foreground)
        .hover(s => s.bg(t.selected)).on_click((_e, cx) => { this.open = null; cx.notify(); })
        .child(div().w(12).flex_none().text_color(t.muted).child("\u2039"))
        .child(div().flex_1().truncate().child(a.name))
        .child(div().text_size(11).text_color(t.dim).child("start on")))
      .child(div().h(1).mx(4).my(5).bg(t.border))
      .child(v_flex().flex_1().min_h(0).overflow_y_scrollbar()
        .children(e.accounts.map((acc, i) => line(`acc-${acc.key}`, acc.starts_here ? "\u2605" : "", acc.name,
          acc.signed_in ? [acc.usage, i < 9 ? String(i + 1) : ""].filter(Boolean).join("   ") : "not signed in", acc.signed_in, () => pick(acc.selector))))
        .child(line("acc-auto", e.start === "auto" ? "\u2605" : "", "Best available", "cswap picks", true, () => pick("auto"))));
  }
  // more: opens the agent's accounts; the arrow at the row's end.
  row(id, icon, name, action, hint = "", enabled = true, more = null) {
    const t = theme();
    return h_flex().id(id).h(31).px(9).gap(10).rounded(7).items_center().flex_none()
      .text_color(enabled ? t.foreground : t.dim).when(enabled, el => el.hover(s => s.bg(t.selected)))
      .on_click(() => { if (enabled) { closeOverlay(props().overlay); action(); } })
      .child(icon).child(div().flex_1().min_w(0).truncate().child(name)).child(div().flex_none().text_size(11).text_color(t.dim).child(hint))
      .when(Boolean(more), el => el.child(div().id(`${id}-accounts`).flex_none().mr(-4).px(5).h(23).flex().items_center().rounded(5)
        .text_color(t.muted).hover(s => s.bg(t.hover).text_color(t.foreground)).tooltip("Choose the account")
        .on_click((_e, cx) => { cx.stop_propagation(); more(); cx.notify(); })
        .child(svg("mods/workspace-chrome/chevron-right.svg").size(12))));
  }
  render() {
    const t = theme();
    const space = spaces().find(s => s.active);
    const installed = this.agents.filter(a => a.installed);
    const choices = this.failed ? installed : [...installed, ...this.agents.filter(a => !a.installed).slice(0, 2)];
    const opened = this.open ? installed.find(a => a.key === this.open) : null;
    if (opened && this.entry(opened)) return this.accounts(t, opened, this.entry(opened));
    const parent = props()?.parent;
    return v_flex().min_h(0).w_full().h(Math.min(491, (parent ? 63 : 167) + Math.max(1, choices.length + (this.failed ? 1 : 0)) * 31)).bg(t.popover).p(5).text_size(12.8)
      .id("new-menu").track_focus(this.handle)
      .on_key_down((e) => {
        if (e.modifiers.control || e.modifiers.platform || e.modifiers.alt) return;
        const agent = /^[1-9]$/.test(e.key) ? installed[Number(e.key) - 1] : null;
        if (!agent) return;
        closeOverlay(props().overlay);
        this.start(agent);
      })
      .child(this.row("terminal", svg("icons/shell.svg").size(14), "Terminal",
        () => parent ? setShared(SPAWN, { id: Date.now(), parent, argv: null, label: "a shell" }) : run("tab.new", { space: space?.id }), parent ? "" : shortcut("tab.new")))
      .child(div().px(9).pt(8).pb(4).text_size(11).text_color(t.dim).child(parent ? `Start a sub-agent of ${parent}` : "Start an agent"))
      .child(v_flex().flex_1().min_h(0).overflow_y_scrollbar()
        .children(choices.map((a, i) => this.row(a.key, a.tint ? svg(a.icon).size(14).text_color(t.foreground) : image(a.icon).size(14), a.name,
          () => this.start(a), a.installed ? [this.startsOn(this.entry(a)), i < 9 ? String(i + 1) : ""].filter(Boolean).join("   ") : "not installed", a.installed,
          a.installed && this.entry(a) ? () => { this.open = a.key; } : null)))
        .when(this.failed || this.agents.length === 0, el => el.child(div().px(9).py(6).text_color(t.dim).child(this.failed ? "Could not check installed agents" : "Checking installed agents…"))))
      .when(!parent, el => el
      .child(div().h(1).mx(4).my(5).bg(t.border))
      .child(this.row("space", div().w(14).child("+"), "New space", () => run("space.new"), shortcut("space.new")))
      .child(this.row("worktree", svg("mods/workspace-chrome/branch.svg").size(14), "New worktree…", () => openOverlay({ mod: "worktree", kind: "dialog", width: 460, height: 280, props: { space: space?.id, folder: panes().find(p => p.focused)?.cwd, action: "new" } }), "", Boolean(space)))
      .child(this.row("worktree-open", svg("mods/workspace-chrome/branch.svg").size(14), "Open worktree\u2026", () => openOverlay({ mod: "worktree", kind: "dialog", width: 560, height: 480, props: { space: space?.id, folder: panes().find(p => p.focused)?.cwd, action: "open" } }), "", Boolean(space))));
  }
}
