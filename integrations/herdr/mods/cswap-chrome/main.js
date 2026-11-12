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
  // A key can open the dashboard too: "<keys>": "cswap-chrome.dashboard" in keys.json.
  init(_props, cx) {
    if (!props()?.footer) return;
    cx.spawn(async cx => {
      for (;;) {
        if ((await nextCommand()) === "dashboard") this.openDashboard(cx);
        cx.notify();
      }
    });
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
    const collapsed = narrow();
    if (props()?.footer) {
      const row = (id, icon, label, dim, action) => h_flex().id(id).h(28).px(8).gap(9).items_center().rounded(8)
        .when(collapsed, el => el.relative().w(32).px(0).justify_center().tooltip(label))
        .text_color(dim ? t.dim : t.muted).hover(s => s.bg(t.hover).text_color(t.foreground)).cursor_pointer()
        .on_click(action).child(svg(icon === "cswap" ? "mods/cswap-chrome/cswap.svg" : `mods/workspace-chrome/${icon}.svg`).size(14))
        .when(!collapsed, el => el.child(label));
      const ready = div().size(6).rounded_full().bg(t.accent);
      return v_flex().size_full().bg(t.chrome).border_t(1).border_color(t.border).px(8).pt(4).pb(8).text_size(13)
        .when(collapsed, el => el.px(0).items_center())
        .child(row("settings", "settings", "Settings", false, () => run("app.settings"))
          .when(Boolean(updates().app.ready), el => el.child(collapsed ? ready.absolute().top(5).right(6) : ready)))
        .child(row("cswap", "cswap", "cswap", false, (_e, cx) => this.openDashboard(cx)))
        .child(row("machine", "plus", "Add machine", true, () => run("mod.open", { name: "machine-add" })));
    }
    return h_flex().size_full().bg(t.chrome).pl(collapsed ? 6 : 14).pr(10).gap(4).items_center().text_size(13.5)
      .when(window()?.platform === "macos" && !window()?.fullscreen && !collapsed, el => el.pl(76))
      .when(!collapsed, el => el.child(h_flex().flex_1().gap(8).items_center().child(image("frame/welcome/herdr.png").size(20).rounded(5)).child(div().font_semibold().text_color(t.foreground).child("Herdr"))))
      .when(!collapsed, el => el.child(this.button("search", svg("mods/workspace-chrome/search.svg").size(15), () => run("mod.open", { name: "picker" }))))
      // On the macOS rail the traffic lights fill the row; the agents list's » unfolds it.
      .when(!(collapsed && window()?.platform === "macos"), el => el.child(this.button("sidebar-toggle", svg("mods/workspace-chrome/sidebar.svg").size(15), toggleSidebar)));
  }
}
