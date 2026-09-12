// 两页共用的渲染核心：配色、主题三档、把一屏 `Response::Screen` 画成
// `<pre>` 里的一串 `<span>`，外加把画布缩进屏幕的那两个测量函数。
//
// **这份东西钉着"两页画的必须是同一件事"这条性质。** 老师那页
// （`page.html`）和学生只读那页（`live.html`）看的是同一块终端画布——配色
// 算错了、字号适配走了样，两页该一起错、一起改好。各留一份的话，迟早只有
// 一页修对了某个渲染 bug，另一页的同一个 bug 要等到下一次上课才被人发现。
//
// **这不是一个真正的模块。** `dct-page` 没有依赖，也搭不出 import/export
// 那一套（也不该搭——见这个 crate 的 `Cargo.toml`）。这段文本被原样接进
// 两页各自的 `<script>`，跟调用方共用同一个函数作用域，靠的是普通的 `var`
// 提升，不是真正的模块系统。所以它对调用方有几条隐含的约定：
//
// 1. 调用方必须在自己的作用域里声明好 `canvasEl`（画布 `<pre>`）、
//    `rulerEl`（量字宽用的尺子）、`screenEl`（画布外层，用来量屏幕可用
//    尺寸）——这几个名字什么时候赋值不要紧（函数只在被调用的那一刻才去
//    找它们），但必须用这几个名字。
// 2. 调用方必须声明好 `zoom`（用户自己调过的字号倍数，`fitFont` 里要用）。
// 3. `paint`/`fitFont`/`cellSize` 都不知道"调用完之后要干什么"（记住这一帧、
//    摆光标、算翻页步长……）——那些是调用方自己的事。

// ——主题三档——
//
// 跟随系统（默认）、浅、深。选过的存 localStorage，**只存在这台设备上**：
// 同一场直播可能同时被好几个人看着，一个人挑的颜色不该跳到另一个人的
// 屏幕上。
var THEMES = ["system", "light", "dark"];
var themeIdx = 0;
try {
  var saved = localStorage.getItem("dct-theme");
  if (THEMES.indexOf(saved) > 0) { themeIdx = THEMES.indexOf(saved); }
} catch (e) {
  // 隐私模式下 localStorage 会抛。选不过是记不住，页面照常能用。
}

// 眼下到底是深还是浅。**跟随系统那一档要去问系统**，不能只看属性：
// 画屏幕那套色号靠它选调色板，看错了整屏字会压在同色背景上。
function isDark() {
  var t = THEMES[themeIdx];
  if (t === "dark") { return true; }
  if (t === "light") { return false; }
  return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

// ——配色——
//
// ANSI 的 0–15 号没有固定的 RGB——终端主题自己定。这里要两套，因为
// **同一套色号在深底和浅底上不可能都读得清**：经典 xterm 那套是为黑底
// 配的，把它压在白底上，11 号亮黄（#ffff00）和 15 号白（#ffffff）直接
// 隐形；反过来浅底那套压在黑底上，0 号黑也一样看不见。
//
// 浅底那套里 7/15 号**故意不是白色**：在黑底终端上它们是默认前景（也就是
// 正文），照字面画成白色的话，一屏正文在白底上全部消失。所有「亮」色也
// 相应压暗——亮色在浅底上的作用是「更重」，不是「更亮」。
var PALETTE = {
  dark: [
    "#000000", "#cd0000", "#00cd00", "#cdcd00",
    "#0000ee", "#cd00cd", "#00cdcd", "#e5e5e5",
    "#7f7f7f", "#ff0000", "#00ff00", "#ffff00",
    "#5c5cff", "#ff00ff", "#00ffff", "#ffffff"
  ],
  light: [
    "#2b2f36", "#a8322a", "#2f6f3e", "#8a5a00",
    "#005f87", "#8b3a8b", "#16697a", "#3a3f4a",
    "#5b6472", "#c0392b", "#3d8b50", "#a06a00",
    "#0a6f9c", "#a34aa3", "#1c7f92", "#14161b"
  ]
};
function idxColor(i) {
  var base = PALETTE[isDark() ? "dark" : "light"];
  if (i < 16) { return base[i]; }
  if (i >= 232) {
    var v = 8 + 10 * (i - 232);
    return "rgb(" + v + "," + v + "," + v + ")";
  }
  var STEP = [0, 95, 135, 175, 215, 255];
  var n = i - 16;
  return "rgb(" + STEP[Math.floor(n / 36)] + "," +
    STEP[Math.floor((n % 36) / 6)] + "," + STEP[n % 6] + ")";
}

// 协议里的颜色是三选一：字符串 "Default"、{Idx:n}、{Rgb:[r,g,b]}。
//
// **判 Idx 必须用 typeof，不能用真值判断**：0 号色是黑色，而 `if (c.Idx)`
// 对 0 是假的——一整屏的黑字会被当成"没上色"。
function cssColor(c) {
  if (!c || c === "Default") { return null; }
  if (typeof c.Idx === "number") { return idxColor(c.Idx); }
  if (c.Rgb) { return "rgb(" + c.Rgb.join(",") + ")"; }
  return null;
}

function styleOf(st) {
  if (!st) { return ""; }
  var fg = cssColor(st.fg);
  var bg = cssColor(st.bg);
  // 反显 = 前景背景对调。哪一边本来没有具体颜色，就拿页面自己的那一档补上——
  // 不补的话，一屏默认色上的"反显"什么都看不出来（而反显正是 dct 自己用来
  // 标"当前项目"的手法）。
  if (st.inverse) {
    var swapped = [bg || "var(--bg)", fg || "var(--ink)"];
    fg = swapped[0];
    bg = swapped[1];
  }
  var out = "";
  if (fg) { out += "color:" + fg + ";"; }
  if (bg) { out += "background:" + bg + ";"; }
  if (st.bold) { out += "font-weight:600;"; }
  if (st.italic) { out += "font-style:italic;"; }
  if (st.underline) { out += "text-decoration:underline;"; }
  return out;
}

// 一整屏画成 <pre> 里的一串 <span>。**用 textContent 塞字**，
// 不拼 HTML 字符串——屏幕上的内容来自 agent，里面什么字节都可能有。
function paint(lines) {
  canvasEl.textContent = "";
  var cols = 0;
  lines.forEach(function (spans, row) {
    var width = 0;
    // **每一行套一个 `<span class="row">`。** 点一行要把这一行的字取出来，
    // 而原来所有 span 是平铺的，点中之后无从知道自己落在第几行。
    // 换行符仍然留在行外面：数行数的地方靠 `canvasEl.textContent` 数，
    // 把 `\n` 收进行里会让那个数字变。
    var line = document.createElement("span");
    line.className = "row";
    spans.forEach(function (sp) {
      var el = document.createElement("span");
      var css = styleOf(sp.style);
      if (css) { el.setAttribute("style", css); }
      el.textContent = sp.text;
      width += sp.text.length;
      line.appendChild(el);
    });
    canvasEl.appendChild(line);
    if (width > cols) { cols = width; }
    if (row < lines.length - 1) {
      canvasEl.appendChild(document.createTextNode("\n"));
    }
  });
  return cols;
}

// 把整块画布**按宽和高一起**缩进屏幕里。
//
// 只按宽度算的话，竖屏上一行放得下、24 行却装不下——而画面是一整块
// 固定尺寸的画布（不改 PTY 尺寸），少看的那几行正是 agent 此刻正在写的
// 那几行。两个方向各算一个上限，取小的那个。
//
// 6px 封底：再小不是"小"，是"看不见"。上限 28px 是给调大的人留的余地——
// 超出屏幕的部分由外层容器自己滚。
function fitFont(cols, rows) {
  if (!cols || !rows) { return; }
  var probe = 16;
  rulerEl.style.fontSize = probe + "px";
  var per = rulerEl.getBoundingClientRect().width / 10;
  if (!per) { return; }

  var padding = 16;
  var byWidth = ((screenEl.clientWidth - padding) / (cols * per)) * probe;
  // 行高是 1.2（见各页 `#screen pre` 的 CSS），高度这一侧要按它折算。
  var byHeight = (screenEl.clientHeight - padding) / (rows * 1.2);
  var size = Math.floor(Math.min(byWidth, byHeight) * zoom);
  canvasEl.style.fontSize = Math.max(6, Math.min(28, size)) + "px";
}

// 一个格子多宽多高。**画光标和认滚轮落在哪个格子上，量的必须是同一个
// 格子**——各量一遍的话，两处迟早在某个字号上错开一列，而错开的那一处
// 是「点在这儿、agent 以为点在旁边」这种没人查得出来的偏差。
function cellSize() {
  var size = parseFloat(canvasEl.style.fontSize) || 16;
  rulerEl.style.fontSize = size + "px";
  return { w: rulerEl.getBoundingClientRect().width / 10, h: size * 1.2 };
}
