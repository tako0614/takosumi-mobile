package com.takosumi.fixture.webhome;

import java.util.ArrayList;
import java.util.List;

/** Existing-JDK control of the exact matcher compiled into the Android driver. */
public final class ConnectionLabelTest {
    private static int checks;

    private static void check(boolean expected, String label, String name, String origin) {
        checks++;
        if (ConnectionLabel.matches(label, name, origin) != expected) {
            throw new AssertionError("Saved connection label control " + checks);
        }
    }

    public static void main(String[] args) {
        String origin = "https://127.0.0.1:41963";
        check(true, "Work " + origin + " 前の画面へ戻る →", "Work", origin);
        check(true, "Work " + origin + " 開く →", "Work", origin);
        check(true, "Work\n" + origin + "\n前の画面へ戻る →", "Work", origin);
        check(true, "Work " + origin, "Work", origin);
        check(true, "Personal " + origin + " 開く →", "Personal", origin);
        check(true, "Team [A]+ " + origin + " 開く →", "Team [A]+", origin);
        for (String label : new String[] {
            "", "Work", "Workを接続先から外す", "保存データを消去…",
            "Work2 " + origin + " 開く →", "Personal " + origin + " 開く →",
            "Work " + origin + "0 開く →", "Work " + origin + "/other 開く →",
            "Work https://127.0.0.2:41963 開く →", "Work https://127x0x0x1:41963 開く →",
            "Work " + origin + " 削除",
            "prefix Work " + origin + " 開く →", "Work " + origin + " 開く → suffix"
        }) check(false, label, "Work", origin);
        check(false, null, "Work", origin);
        check(false, "Work " + origin, "", origin);
        check(false, "Work " + origin, "Work", "");
        check(false, "Team AAAAA " + origin, "Team [A]+", origin);
        rowControls();
        System.out.println("PASS: saved connection label checks=" + checks);
    }

    private static final String PKG = "com.takosumi.fixture.android";
    private static final String ORIGIN = "https://127.0.0.1:39161";
    private static AccessibleName.Candidate value(String pkg, String text, String description,
            boolean enabled, boolean clickable, boolean visible, int l, int t, int r, int b) {
        return new AccessibleName.Candidate(pkg, text, description, enabled, clickable, visible,
            AccessibleName.MISSING, l, t, r, b);
    }
    private static ConnectionLabel.Node node(Object id, Object parent, String type, String text,
            boolean clickable, int l, int t, int r, int b) {
        return new ConnectionLabel.Node(id, parent, type, value(PKG, text, "", true, clickable, true, l, t, r, b));
    }
    private static List<ConnectionLabel.Node> rows() {
        Object root = new Object(), work = new Object(), personal = new Object();
        List<ConnectionLabel.Node> n = new ArrayList<>();
        n.add(node(root, null, "android.webkit.WebView", "", false, 0, 0, 1080, 2400));
        n.add(node(work, root, "android.view.View", "", false, 63, 955, 1021, 1428));
        n.add(node(new Object(), work, "android.widget.Button", "Work " + ORIGIN + " 前の画面へ戻る →", true, 63, 955, 1021, 1304));
        n.add(node(new Object(), work, "android.widget.Button", "Workを接続先から外す", true, 63, 1302, 359, 1428));
        n.add(node(new Object(), work, "android.widget.Button", ConnectionLabel.CLEAR, true, 406, 1302, 790, 1428));
        n.add(node(personal, root, "android.view.View", "", false, 63, 1467, 1021, 1942));
        n.add(node(new Object(), personal, "android.widget.Button", "Personal " + ORIGIN + " 開く →", true, 63, 1467, 1021, 1816));
        n.add(node(new Object(), personal, "android.widget.Button", "Personalを接続先から外す", true, 63, 1813, 359, 1942));
        n.add(node(new Object(), personal, "android.widget.Button", ConnectionLabel.CLEAR, true, 406, 1813, 790, 1942));
        return n;
    }
    private static ConnectionLabel.Node changedValue(ConnectionLabel.Node n, AccessibleName.Candidate v) {
        return new ConnectionLabel.Node(n.identity, n.parent, n.className, v);
    }
    private static List<ConnectionLabel.Node> replace(List<ConnectionLabel.Node> source, int i, ConnectionLabel.Node n) {
        List<ConnectionLabel.Node> copy = new ArrayList<>(source); copy.set(i, n); return copy;
    }
    private static void require(boolean condition) {
        checks++; if (!condition) throw new AssertionError("Profile-bound clear control " + checks);
    }
    private static void refuse(List<ConnectionLabel.Node> n) {
        require(ConnectionLabel.selectClear(n, PKG, "Work", ORIGIN) == null);
    }
    private static void rowControls() {
        List<ConnectionLabel.Node> n = rows();
        ConnectionLabel.Selection work = ConnectionLabel.selectClear(n, PKG, "Work", ORIGIN);
        ConnectionLabel.Selection personal = ConnectionLabel.selectClear(n, PKG, "Personal", ORIGIN);
        require(work != null && work.clear.identity == n.get(4).identity);
        require(personal != null && personal.clear.identity == n.get(8).identity);
        require(work.clear.identity != personal.clear.identity);
        List<AccessibleName.Candidate> generic = new ArrayList<>();
        for (ConnectionLabel.Node entry : n) generic.add(entry.value);
        require(AccessibleName.unique(generic, PKG, ConnectionLabel.CLEAR) == AccessibleName.AMBIGUOUS);
        require(ConnectionLabel.rebind(work, new ArrayList<>(n), PKG, "Work", ORIGIN) != null);
        require(ConnectionLabel.rebind(work, n, PKG, "Personal", ORIGIN) == null);
        require(ConnectionLabel.rebind(work, n, PKG, "Work", ORIGIN + "/other") == null);
        require(ConnectionLabel.selectClear(n, PKG, "Other", ORIGIN) == null);
        require(ConnectionLabel.selectClear(n, "foreign", "Work", ORIGIN) == null);
        require(ConnectionLabel.selectClear(n, PKG, "Work", "https://127.0.0.2:39161") == null);
        for (int i : new int[] {0, 1, 2, 3, 4}) {
            ConnectionLabel.Node x = n.get(i); AccessibleName.Candidate v = x.value;
            refuse(replace(n, i, changedValue(x, value("foreign", v.text, v.description, true, v.clickable, true, v.left, v.top, v.right, v.bottom))));
            refuse(replace(n, i, changedValue(x, value(PKG, v.text, v.description, false, v.clickable, true, v.left, v.top, v.right, v.bottom))));
            refuse(replace(n, i, changedValue(x, value(PKG, v.text, v.description, true, v.clickable, false, v.left, v.top, v.right, v.bottom))));
            refuse(replace(n, i, changedValue(x, value(PKG, v.text, v.description, true, v.clickable, true, v.left, v.top, v.left, v.bottom))));
        }
        for (int i : new int[] {2, 3, 4}) {
            ConnectionLabel.Node x = n.get(i); AccessibleName.Candidate v = x.value;
            refuse(replace(n, i, changedValue(x, value(PKG, v.text, v.description, true, false, true, v.left, v.top, v.right, v.bottom))));
        }
        for (int i : new int[] {1, 2, 3, 4}) {
            ConnectionLabel.Node x = n.get(i);
            refuse(replace(n, i, new ConnectionLabel.Node(x.identity, null, x.className, x.value)));
            require(ConnectionLabel.rebind(work, replace(n, i, new ConnectionLabel.Node(new Object(), x.parent, x.className, x.value)), PKG, "Work", ORIGIN) == null);
        }
        ConnectionLabel.Node card = n.get(2), clear = n.get(4), row = n.get(1), root = n.get(0);
        refuse(replace(n, 2, changedValue(card, value(PKG, card.value.text, "Personal " + ORIGIN, true, true, true, 63, 955, 1021, 1304))));
        refuse(replace(n, 4, changedValue(clear, value(PKG, ConnectionLabel.CLEAR, "different action", true, true, true, 406, 1302, 790, 1428))));
        refuse(replace(n, 4, changedValue(clear, value(PKG, ConnectionLabel.CLEAR, "", true, true, true, 406, 1302, 1100, 1428))));
        ConnectionLabel.Node remove = n.get(3);
        refuse(replace(n, 3, changedValue(remove, value(PKG, remove.value.text, "", true, true, true, 63, 1302, 500, 1428))));
        List<ConnectionLabel.Node> duplicate = new ArrayList<>(n);
        duplicate.add(new ConnectionLabel.Node(new Object(), clear.parent, clear.className, clear.value)); refuse(duplicate);
        duplicate = new ArrayList<>(n); duplicate.add(new ConnectionLabel.Node(new Object(), card.parent, card.className, card.value)); refuse(duplicate);
        duplicate = new ArrayList<>(n); duplicate.add(new ConnectionLabel.Node(row.identity, row.parent, row.className, row.value)); refuse(duplicate);
        List<ConnectionLabel.Node> shared = new ArrayList<>(n);
        for (int i : new int[] {2, 3, 4, 6, 7, 8}) {
            ConnectionLabel.Node x = n.get(i); shared.set(i, new ConnectionLabel.Node(x.identity, root.identity, x.className, x.value));
        }
        refuse(shared);
        refuse(replace(n, 8, new ConnectionLabel.Node(n.get(8).identity, row.identity, n.get(8).className, n.get(8).value)));
        refuse(replace(n, 0, new ConnectionLabel.Node(root.identity, row.identity, root.className, root.value)));
        require(ConnectionLabel.rebind(work, replace(n, 4, new ConnectionLabel.Node(clear.identity, n.get(5).identity, clear.className, clear.value)), PKG, "Work", ORIGIN) == null);
        ConnectionLabel.Node altered = changedValue(clear, value(PKG, "保存データを消去", "", true, true, true, 406, 1302, 790, 1428));
        require(ConnectionLabel.rebind(work, replace(n, 4, altered), PKG, "Work", ORIGIN) == null);
    }
}
