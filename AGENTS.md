# Rust 编码规范 — 防越界 / 防 panic

本文件固化本仓库（Verve，基于 GPUI 的 Rust 桌面应用）的索引与切片规范，目标是**消灭 `index out of bounds`、`byte index … is not a char boundary`、`called Option::unwrap() on a None` 这三类 panic**。写代码或改代码前先按此执行。

> 背景：本应用面向中文等多字节字符场景（SSH/Markdown/JSON 内容、token 显示等都含非 ASCII），UTF-8 字符串切片是高频 panic 源。

---

## 一、集合索引：禁止裸 `[i]`，统一用 `.get(i)`

**规则**：访问 `Vec / slice / [T]` 时，**一律用 `get(i)` 返回 `Option`，再用 `let-else` 或 `match` 处理**；禁止写 `arr[i]`（直接 panic 形式），除非索引在**同一作用域内已被证明在界内**（如 `for (i, x) in v.iter().enumerate()` 内部用 `v[i]`、或紧接着 `if i < v.len()`）。

✅ 推荐：

```rust
// json_panel.rs 的 render 闭包里曾经是 rows[ix]（越界即崩），应写成：
let Some(row) = rows.get(ix) else { return div().into_any_element() };
render_row(ix, row, &mono_font, fg, /* … */, weak.clone())
```

```rust
// 已正确的范例（docker_panel.rs:1051）：
if let Some(s) = this.sessions.get(idx) { /* … */ }
```

❌ 禁止：

```rust
let rt = self.sessions[idx].input_tx.take();   // idx 可能越界 → panic
self.tabs[idx].disk_mtime = …;                  // 同上
```

**多元素访问**用 `get(i..j)` / `split_first()` / `split_last()`，不要 `[i..j]`。

---

## 二、字符串切片：禁止按字节下标切，必须校验字符边界

**规则**：对 `&str` / `String` 切片时，下标必须是**字符边界**；任何由 `find`、`len()`、固定常量（如 `..6`、`len()-4`）算出来的下标，**切之前要么用 `is_char_boundary()` 校验，要么改用 `.get(range)` / `.char_indices()`**。

⚠️ 高危真实案例（本仓库现存）：

```rust
// project_manage_panel.rs:2176 —— token 若含多字节字符或 < 4 字节会 panic
let masked = if t.token.len() > 12 {
    format!("{}••••••{}", &t.token[..6], &t.token[t.token.len() - 4..]) // ❌
} else { … };
```
应改为按字符计数并校验边界：

```rust
// ✅ 按字符数截断，且用 .get(..n) 安全切片
let chars: Vec<char> = t.token.chars().collect();
let masked = if chars.len() > 12 {
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars.iter().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}••••••{tail}")
} else {
    t.token.clone()
};
```

```rust
// hosts.rs:55 —— '#' 由 find 给出的是字节下标，对 ASCII '#' 是边界安全，但 line[i+1..] 依赖 i+1 也是边界。
// '#' 必为单字节 ASCII，故此处安全；但新增类似逻辑（用 find('中文字符')）时务必改用 char_indices。
```

```rust
// updater.rs:235 —— notes[..2000] 当 2000 落在多字节字符中间会 panic
format!("{}…", notes.get(..2000).unwrap_or(notes)) // ✅ 用 get
```

**速记**：`&s[a..b]` → 先问自己「a、b 一定是字符边界吗？」拿不准就 `s.get(a..b)`。

---

## 三、`unwrap()` / `expect()`：生产代码默认禁用

**规则**：`src/` 下的**非测试代码**禁止 `.unwrap()` / `.expect()`，除非满足下列**豁免条件之一**：

1. **编译期常量**：`serde_json::to_string(&json!({}))` 这类「输入合法、绝不失败」的序列化，可 `unwrap()`，但**加注释说明为何安全**。
2. **紧邻的非空断言**：上一行刚 `if !v.is_empty()` 或 `if let Some(x)`，且编译器/逻辑可证。

其余一律改为：
- `?` 传播（函数返回 `Result`）；
- `.unwrap_or(默认值)` / `.unwrap_or_default()`；
- `if let Some / let … else` 显式处理。

测试代码（`#[cfg(test)]`、`tests/`）可自由 `unwrap()`。

> 现状：`src/` 非测试代码有约 460 处 `.unwrap()/.expect()`，后续重构逐步替换；**新代码不要再新增**。

---

## 四、整数运算：警惕溢出与负数下标

- 下标运算前判断：`usize` 减法可能下溢（`i - 1` 当 `i == 0` → panic in debug、wrap in release）。用 `i.checked_sub(1)` 或 `i.saturating_sub(1)`。
- 数值转下标 `as usize` 前，确保非负且在界内（先校验再转）。
- 长度比较走 `.len()`，不要拿外部传入的 `i32/u32` 直接当索引。

---

## 五、CLI / UI 回调：入口处先校验

GPUI 的 `on_click` / `on_action` / `update(cx, …)` 闭包、以及任何接收外部索引（当前 tab、选中行、列表可见区间）的回调，**第一步就是校验索引在界内**，越界直接 `return`，绝不让异常冒泡到渲染线程导致整个窗口 panic。

```rust
.on_click({
    let weak = weak.clone();
    move |_ev, _window, cx| {
        let _ = weak.update(cx, |this, cx| {
            let Some(item) = this.items.get(i) else { return }; // ✅ 先校验
            this.activate(item, cx);
        });
    }
})
```

---

## 六、虚拟列表：行数与数据同步

`uniform_list` / `list` 的回调拿到的是「可见区间下标」，它可能**短暂大于你的数据长度**（数据刚变、列表尚未重置）。规则：
- 回调里 `rows.get(ix)` 而非 `rows[ix]`；
- 数据变更后**立刻** `list_state.reset(new_len)`，保证列表内部缓存与新数据一致（见 `json_panel.rs::rebuild_rows`）。

---

## 七、Review 自检清单（提交前过一遍）

- [ ] 是否存在 `arr[i]` / `s[a..b]` 裸索引/切片？→ 改 `get`。
- [ ] 字符串切片的下标是否一定是 UTF-8 字符边界？→ 拿不准用 `get` 或 `char_indices`。
- [ ] 是否新增了 `.unwrap()` / `.expect()`？→ 不在豁免条件内就改掉。
- [ ] 下标是否由减法 / 外部输入 / 类型转换得到？→ `checked_sub` + 范围校验。
- [ ] UI 回调入口是否对索引做了边界校验？→ 先 `get` 再操作。
- [ ] 改动了列表数据源后，是否调用了 `reset` / 重建？→ 同步状态。

---

*本规范随代码演进，发现新的 panic 模式请在此补充对应规则与案例。*
