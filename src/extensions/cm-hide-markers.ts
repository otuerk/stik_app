/**
 * Live preview — Obsidian/Zettlr-style markdown marker hiding.
 *
 * Uses Decoration.replace({}) to hide syntax markers (**, *, ~~, ==, `, #, >)
 * always except when the cursor is directly on the marker characters themselves.
 * This prevents the "bouncing" effect where moving the cursor through a heading
 * or link causes the raw syntax to flash in and out.
 *
 * Pattern from Zettlr (12.5k stars) and SilverBullet (4.7k stars):
 *   1. Iterate syntax tree within view.visibleRanges (viewport only)
 *   2. For each formatting node, check rangeInSelection()
 *   3. If cursor outside -> Decoration.replace({}) on the marker children
 *   4. Rebuild on docChanged | viewportChanged | selectionSet
 */

import {
  ViewPlugin,
  Decoration,
  EditorView,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { syntaxTree } from "@codemirror/language";
import type { Extension, Range } from "@codemirror/state";
import { rangeInSelection } from "./cm-range-utils";

/** Reusable empty replacement — hides a range with no visual substitute */
const hiddenDeco = Decoration.replace({});

/**
 * Scan visible portions of the syntax tree and collect Decoration.replace({})
 * ranges for all formatting markers whose parent node is NOT under the cursor.
 */
function buildDecorations(view: EditorView): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  const { state } = view;
  const { doc, selection } = state;

  for (const { from, to } of view.visibleRanges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter(node) {
        switch (node.name) {
          // ── Inline formatting ──────────────────────────────────────

          case "StrongEmphasis":
          case "Emphasis": {
            // Reveal only the marker the cursor is directly on, not the whole node.
            // This stops ** bouncing when the cursor moves through formatted text.
            for (const mark of node.node.getChildren("EmphasisMark")) {
              if (!rangeInSelection(selection, mark.from, mark.to)) {
                ranges.push(hiddenDeco.range(mark.from, mark.to));
              }
            }
            break;
          }

          case "Strikethrough": {
            for (const mark of node.node.getChildren("StrikethroughMark")) {
              if (!rangeInSelection(selection, mark.from, mark.to)) {
                ranges.push(hiddenDeco.range(mark.from, mark.to));
              }
            }
            break;
          }

          case "InlineCode": {
            for (const mark of node.node.getChildren("CodeMark")) {
              if (!rangeInSelection(selection, mark.from, mark.to)) {
                ranges.push(hiddenDeco.range(mark.from, mark.to));
              }
            }
            break;
          }

          case "Link": {
            // Reveal brackets/parens only when cursor is directly on them,
            // and URL only when cursor is inside the URL itself.
            for (const mark of node.node.getChildren("LinkMark")) {
              if (!rangeInSelection(selection, mark.from, mark.to)) {
                ranges.push(hiddenDeco.range(mark.from, mark.to));
              }
            }
            for (const url of node.node.getChildren("URL")) {
              if (!rangeInSelection(selection, url.from, url.to)) {
                ranges.push(hiddenDeco.range(url.from, url.to));
              }
            }
            break;
          }

          // Image nodes are fully handled by the block widget plugin —
          // don't hide sub-markers here, it would conflict with the replace decoration.
          case "Image":
            return false;

          // Custom ==highlight== extension
          case "Highlight": {
            for (const mark of node.node.getChildren("HighlightMark")) {
              if (!rangeInSelection(selection, mark.from, mark.to)) {
                ranges.push(hiddenDeco.range(mark.from, mark.to));
              }
            }
            break;
          }

          // ── Block markers ──────────────────────────────────────────

          // Heading: # ## ### — only reveal when cursor is on the # itself.
          case "HeaderMark": {
            if (!node.node.parent) break;
            if (rangeInSelection(selection, node.from, node.to)) break;
            // Include trailing space after # in the hidden range
            let end = node.to;
            if (end < doc.length && doc.sliceString(end, end + 1) === " ") {
              end += 1;
            }
            ranges.push(hiddenDeco.range(node.from, end));
            break;
          }

          // Blockquote: > — only reveal when cursor is on the > itself.
          case "QuoteMark": {
            if (rangeInSelection(selection, node.from, node.to)) break;
            // Hide > and optional trailing space
            const slice = doc.sliceString(node.from, node.from + 2);
            const match = /^>[ ]?/.exec(slice);
            const end = match ? node.from + match[0].length : node.to;
            ranges.push(hiddenDeco.range(node.from, end));
            break;
          }
        }
      },
    });
  }

  // Empty auto-closed pairs at cursor (**** ~~ ====).
  // The parser can't recognize these as formatting (no content between markers).
  // Hide both marker pairs so the user sees a clean insertion point.
  for (const sel of selection.ranges) {
    const pos = sel.head;
    if (pos >= 2 && pos + 2 <= doc.length) {
      const around = doc.sliceString(pos - 2, pos + 2);
      if (around === "****" || around === "~~~~" || around === "====") {
        ranges.push(hiddenDeco.range(pos - 2, pos));
        ranges.push(hiddenDeco.range(pos, pos + 2));
      }
    }
  }

  return Decoration.set(ranges, true);
}

const hideMarkersViewPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged || update.selectionSet) {
        this.decorations = buildDecorations(update.view);
      }
    }
  },
  { decorations: (v) => v.decorations },
);

export const hideMarkersPlugin: Extension = hideMarkersViewPlugin;

/* ── Auto-close paired markers ── */

export const autoCloseMarkup = EditorView.inputHandler.of(
  (view, from, to, text) => {
    if (text.length !== 1 || from !== to) return false;

    const doc = view.state.doc;
    const before = from > 0 ? doc.sliceString(from - 1, from) : "";
    const beforeBefore = from > 1 ? doc.sliceString(from - 2, from - 1) : "";
    const after = doc.sliceString(from, Math.min(from + 2, doc.length));

    // Skip over closing markers when exiting formatted text.
    // e.g. cursor in **hello|** — typing * jumps past the closing **
    if (
      (text === "*" && after.startsWith("**") && before !== "*") ||
      (text === "~" && after.startsWith("~~") && before !== "~") ||
      (text === "=" && after.startsWith("==") && before !== "=")
    ) {
      view.dispatch({ selection: { anchor: from + 2 } });
      return true;
    }

    // Auto-close: when typing the second char of a pair (** ~~ ==),
    // insert the typed char + closing pair and place cursor between.
    if (text === "*" && before === "*" && beforeBefore !== "*" && !after.startsWith("*")) {
      view.dispatch({
        changes: { from, insert: "***" },  // typed * + closing **
        selection: { anchor: from + 1 },
      });
      return true;
    }
    if (text === "~" && before === "~" && beforeBefore !== "~" && !after.startsWith("~")) {
      view.dispatch({
        changes: { from, insert: "~~~" },
        selection: { anchor: from + 1 },
      });
      return true;
    }
    if (text === "=" && before === "=" && beforeBefore !== "=" && !after.startsWith("=")) {
      view.dispatch({
        changes: { from, insert: "===" },
        selection: { anchor: from + 1 },
      });
      return true;
    }

    return false;
  },
);
