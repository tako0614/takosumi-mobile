package com.takosumi.fixture.webhome;

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
        System.out.println("PASS: saved connection label checks=" + checks);
    }
}
