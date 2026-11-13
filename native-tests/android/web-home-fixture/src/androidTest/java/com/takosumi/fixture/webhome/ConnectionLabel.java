package com.takosumi.fixture.webhome;

import java.util.regex.Pattern;
import java.util.List;

/** The WebView's merged accessible name for a saved connection button. */
final class ConnectionLabel {
    private ConnectionLabel() { }

    static boolean matches(String label, String name, String origin) {
        if (label == null || name.isEmpty() || origin.isEmpty()) return false;
        String value = Pattern.quote(name) + "\\s+" + Pattern.quote(origin)
            + "(?:\\s+(?:開く|前の画面へ戻る)\\s+→)?";
        return Pattern.compile(value).matcher(label).matches();
    }

    static final String CLEAR = "保存データを消去…";

    /** Identity/parent are actual UiObject2 handles at the driver boundary. */
    static final class Node {
        final Object identity, parent;
        final String className;
        final AccessibleName.Candidate value;
        Node(Object identity, Object parent, String className, AccessibleName.Candidate value) {
            this.identity = identity; this.parent = parent; this.className = className; this.value = value;
        }
    }

    static final class Selection {
        final Node row, card, remove, clear;
        final String targetPackage, name, origin;
        private Selection(Node row, Node card, Node remove, Node clear,
                          String targetPackage, String name, String origin) {
            this.row = row; this.card = card; this.remove = remove; this.clear = clear;
            this.targetPackage = targetPackage; this.name = name; this.origin = origin;
        }
    }

    private static boolean same(Object a, Object b) { return a != null && b != null && a.equals(b); }
    private static boolean empty(String value) { return value == null || value.isEmpty(); }
    private static Node identity(List<Node> nodes, Object id) {
        Node found = null;
        if (id == null) return null;
        for (Node node : nodes) if (node != null && same(node.identity, id)) {
            if (found != null) return null;
            found = node;
        }
        return found;
    }
    private static boolean ready(Node node, String targetPackage) {
        if (node == null || node.identity == null || node.value == null) return false;
        AccessibleName.Candidate v = node.value;
        return targetPackage.equals(v.packageName) && v.enabled && v.visible
            && v.left < v.right && v.top < v.bottom;
    }
    private static boolean button(Node node, String targetPackage) {
        return ready(node, targetPackage) && "android.widget.Button".equals(node.className) && node.value.clickable;
    }
    private static boolean contains(Node parent, Node child) {
        AccessibleName.Candidate a = parent.value, b = child.value;
        return a.left <= b.left && b.right <= a.right && a.top <= b.top && b.bottom <= a.bottom;
    }
    private static boolean cardName(Node node, String name, String origin) {
        if (node == null || node.value == null) return false;
        return matches(node.value.text, name, origin) || matches(node.value.description, name, origin);
    }

    static Selection selectClear(List<Node> nodes, String targetPackage, String name, String origin) {
        if (nodes == null || empty(targetPackage) || empty(name) || empty(origin)) return null;
        Node card = null;
        for (Node node : nodes) if (cardName(node, name, origin)) {
            if (card != null) return null;
            card = node;
        }
        if (!button(card, targetPackage) || identity(nodes, card.identity) != card) return null;
        AccessibleName.Candidate v = card.value;
        if ((!empty(v.text) && !matches(v.text, name, origin))
                || (!empty(v.description) && !matches(v.description, name, origin))
                || (!empty(v.text) && !empty(v.description) && !v.text.equals(v.description))) return null;
        Node row = identity(nodes, card.parent);
        if (!ready(row, targetPackage) || !"android.view.View".equals(row.className)
                || row.value.clickable || !empty(row.value.text) || !empty(row.value.description)
                || same(row.identity, card.identity)) return null;
        Node outer = identity(nodes, row.parent);
        if (!ready(outer, targetPackage) || same(outer.identity, row.identity)
                || same(outer.parent, row.identity) || !contains(outer, row)) return null;
        Node remove = null, clear = null;
        int children = 0;
        for (Node node : nodes) if (node != null && same(node.parent, row.identity)) {
            children++;
            if (!button(node, targetPackage) || !contains(row, node)
                    || identity(nodes, node.identity) != node) return null;
            if (node == card) continue;
            if (AccessibleName.matches(node.value, targetPackage, name + "を接続先から外す")) {
                if (remove != null) return null;
                remove = node;
            } else if (AccessibleName.matches(node.value, targetPackage, CLEAR)) {
                if (clear != null) return null;
                clear = node;
            } else return null;
        }
        if (children != 3 || remove == null || clear == null) return null;
        AccessibleName.Candidate a = remove.value, b = clear.value;
        if (a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom) return null;
        return new Selection(row, card, remove, clear, targetPackage, name, origin);
    }

    static Selection rebind(Selection held, List<Node> current, String targetPackage, String name, String origin) {
        if (held == null || !held.targetPackage.equals(targetPackage)
                || !held.name.equals(name) || !held.origin.equals(origin)) return null;
        Selection fresh = selectClear(current, targetPackage, name, origin);
        if (fresh == null) return null;
        Node[] before = { held.row, held.card, held.remove, held.clear };
        Node[] after = { fresh.row, fresh.card, fresh.remove, fresh.clear };
        for (int n = 0; n < before.length; n++) {
            if (!same(before[n].identity, after[n].identity) || !same(before[n].parent, after[n].parent)) return null;
        }
        return fresh;
    }
}
