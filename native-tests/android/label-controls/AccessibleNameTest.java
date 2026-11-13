package com.takosumi.fixture.webhome;

import java.util.Arrays;
import java.util.Collections;

/** Small control of the policy actually compiled into the external UI driver. */
public final class AccessibleNameTest {
    private static final String TARGET = "com.takosumi.fixture.android";
    private static int checks;

    private static AccessibleName.Candidate node(String text, String description) {
        return new AccessibleName.Candidate(TARGET, text, description, true, true, true);
    }

    private static AccessibleName.Candidate notes(boolean clickable, int parent, int left, int right) {
        return new AccessibleName.Candidate(TARGET, clickable ? "" : "📝 Notes", clickable ? "📝 Notes" : "",
            true, clickable, true, parent, left, 747, right, 818);
    }

    private static void select(int expected, String label, AccessibleName.Candidate... values) {
        checks++;
        if (AccessibleName.unique(Arrays.asList(values), TARGET, label) != expected) {
            throw new AssertionError("Accessible name control " + checks);
        }
    }

    public static void main(String[] args) {
        // Reduced attributes of the recorded Account settings node: empty
        // text, exact description, current package, visible/enabled/clickable.
        select(0, "Account settings", node("", "Account settings"));
        select(0, "Account settings", node("Account settings", null));
        select(0, "Account settings", node("Account settings", "Account settings"));
        select(0, "Account settings", node(null, "Account settings"));
        select(0, "📝 Notes", node("📝 Notes", ""));
        select(0, "保存データを消去", node("", "保存データを消去"));
        select(-1, "Account settings", node("Account settings Personal", "Account settings"));
        select(-1, "Account settings", node("Account settings", "Account settings Personal"));
        select(-1, "Account settings", node("Personal Account settings", ""));
        select(-1, "Account settings", node("", "Account settings "));
        select(-1, "Account settings", node("account settings", null));
        select(-1, "Account settings", node("", "prefix Account settings"));
        select(-1, "Account settings", node("", ""));
        select(-1, "Account settings", node(null, null));
        select(-1, "Account settings", new AccessibleName.Candidate("other.app", "", "Account settings", true, true, true));
        select(-1, "Account settings", new AccessibleName.Candidate(TARGET, "", "Account settings", false, true, true));
        select(0, "Account settings", new AccessibleName.Candidate(TARGET, "", "Account settings", true, false, true));
        select(-1, "Account settings", new AccessibleName.Candidate(TARGET, "", "Account settings", true, true, false));
        select(1, "Account settings",
            new AccessibleName.Candidate(TARGET, "", "Account settings", true, true, false),
            node("", "Account settings"));
        select(1, "Account settings", node("", "Personal settings"), node("Account settings", ""));
        select(AccessibleName.AMBIGUOUS, "Account settings", node("", "Account settings"), node("Account settings", ""));
        select(AccessibleName.AMBIGUOUS, "Account settings", node("Account settings", "Account settings"), node("", "Account settings"));
        select(-1, "Account settings");
        select(-1, "", node("", ""));
        select(-1, null, node("Account settings", ""));
        checks++;
        if (AccessibleName.matches(node("Account settings", ""), "", "Account settings")) throw new AssertionError("empty package");
        checks++;
        if (AccessibleName.unique(Collections.singletonList(null), TARGET, "Account settings") != -1) throw new AssertionError("null node");
        // The existing click(String) also operates native PopupMenu labels;
        // the text child need not itself carry the parent's click listener.
        select(0, "アプリホームへ", new AccessibleName.Candidate(TARGET, "アプリホームへ", null, true, false, true));
        select(-1, "アプリホームへ", new AccessibleName.Candidate(TARGET, "アプリホームへ", null, false, false, true));
        select(-1, "アプリホームへ", new AccessibleName.Candidate(TARGET, "アプリホームへ", null, true, false, false));
        select(-1, "アプリホームへ", new AccessibleName.Candidate("other.app", "アプリホームへ", null, true, false, true));
        select(-1, "アプリホームへ", new AccessibleName.Candidate(TARGET, "Personal", "アプリホームへ", true, false, true));
        // Actual recorded Notes shape: the text-only direct child is inside
        // its desc-only clickable parent. The adapter now retains that edge.
        select(0, "📝 Notes", notes(true, -1, 63, 1021), notes(false, 0, 63, 304));
        select(1, "📝 Notes", notes(false, 1, 63, 304), notes(true, -1, 63, 1021));
        // Unknown/separate trees and coincident geometry are not an edge.
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, -1, 63, 1021), notes(false, -1, 63, 304));
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, -1, 63, 1021), notes(false, -1, 63, 1021));
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, -1, 63, 1021), notes(false, 99, 63, 304));
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, -1, 63, 1021), notes(false, 1, 63, 304));
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, 1, 63, 1021), notes(false, 0, 63, 304));
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, 0, 63, 1021), notes(false, 0, 63, 304));
        // A nested independent click target must remain a separate action.
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, -1, 63, 1021), notes(true, 0, 63, 304));
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, -1, 63, 1021), notes(false, 0, 0, 304));
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, -1, 63, 1021), notes(false, 0, 63, 304), notes(true, -1, 63, 1021));
        // Separate siblings inside an unrelated container are not parent/child.
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(true, 2, 63, 1021), notes(false, 2, 63, 304),
            new AccessibleName.Candidate(TARGET, "container", "", true, false, true, -1, 0, 0, 1080, 2300));
        select(AccessibleName.AMBIGUOUS, "📝 Notes", notes(false, -1, 63, 1021), notes(false, 0, 63, 304));
        select(0, "📝 Notes", notes(true, -1, 63, 1021), notes(false, 0, 63, 1021));
        System.out.println("PASS: exact accessible name checks=" + checks);
    }
}
