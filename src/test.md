# Markdown Rendering Test Suite

This fixture exercises a broad mix of supported Markdown constructs alongside
sections that used to fall back to raw Markdown. It keeps the renderer's import
+ round-trip behavior under test.

## Headings and prose

Introductory paragraph used as padding so the visible-block count stays high.

### Subsection

Another paragraph of plain prose for visible-block padding.

#### Level four

Padding paragraph.

##### Level five

Padding paragraph.

###### Level six

Padding paragraph.

## Quotes

> Blockquote paragraph one.
> quoted paragraph two

> Standalone second quote for good measure.

## Fenced code

```rust
println!("fenced code block");
```

```js
let x = 1;
```

## Tasks and lists

- [ ] Unchecked task item
- [x] Checked task item
- Mixed list item
- Another bullet
1. ordered one
2. ordered two

## Inline code edge cases

Code span across line breaks:
`line 1
line 2`

Backticks in normal text: `` ` `` and ``` `` ``` and ```` ``` ````

## Table

| A | B | C |
|---|---|---|
| 1 | 2 | 3 |
| 4 | 5 | 6 |

## HTML

<details><summary> collapsible </summary> body </details>

## More padding

Padding paragraph one.

Padding paragraph two.

Padding paragraph three.

Padding paragraph four.

Padding paragraph five.

Padding paragraph six.

Padding paragraph seven.

Padding paragraph eight.

Padding paragraph nine.

Padding paragraph ten.
