package com.takosumi.fixture.webhome;

import java.util.List;

/** Exact visible action-name selection, shared by the real driver and JDK controls. */
final class AccessibleName {
    static final int MISSING = -1;
    static final int AMBIGUOUS = -2;

    static final class Candidate {
        final String packageName, text, description;
        final boolean enabled, clickable, visible;
        final int parentIndex, left, top, right, bottom;

        Candidate(String packageName, String text, String description,
                  boolean enabled, boolean clickable, boolean visible) {
            this(packageName, text, description, enabled, clickable, visible, MISSING, 0, 0, 0, 0);
        }

        Candidate(String packageName, String text, String description,
                  boolean enabled, boolean clickable, boolean visible,
                  int parentIndex, int left, int top, int right, int bottom) {
            this.packageName = packageName;
            this.text = text;
            this.description = description;
            this.enabled = enabled;
            this.clickable = clickable;
            this.visible = visible;
            this.parentIndex = parentIndex;
            this.left = left;
            this.top = top;
            this.right = right;
            this.bottom = bottom;
        }

        Candidate withParent(int index) {
            return new Candidate(packageName, text, description, enabled, clickable, visible,
                index, left, top, right, bottom);
        }
    }

    private AccessibleName() { }

    static boolean matches(Candidate value, String targetPackage, String label) {
        if (value == null || targetPackage == null || targetPackage.isEmpty()
                || label == null || label.isEmpty() || !targetPackage.equals(value.packageName)
                || !value.enabled || !value.visible) return false;
        // Clickable is observed metadata, not a readiness fence: a visible
        // menu label can deliver its coordinate gesture to the parent row.
        boolean text = value.text != null && !value.text.isEmpty();
        boolean description = value.description != null && !value.description.isEmpty();
        // A nonempty conflicting carrier is not the same displayed action.
        return (text || description) && (!text || label.equals(value.text))
            && (!description || label.equals(value.description));
    }

    static int unique(List<Candidate> values, String targetPackage, String label) {
        int selected = MISSING;
        for (int n = 0; n < values.size(); n++) {
            if (!matches(values.get(n), targetPackage, label)) continue;
            int action = actionOwner(values, n, targetPackage, label);
            if (selected != MISSING && selected != action) return AMBIGUOUS;
            selected = action;
        }
        return selected;
    }

    /** parentIndex must come from actual direct-parent node identity, never
     * matching names/bounds alone. Nonclickable standalone labels stay valid.
     */
    private static int actionOwner(List<Candidate> values, int index, String targetPackage, String label) {
        Candidate child = values.get(index);
        int parentIndex = child.parentIndex;
        if (child.clickable || parentIndex < 0 || parentIndex >= values.size() || parentIndex == index) return index;
        Candidate parent = values.get(parentIndex);
        if (matches(parent, targetPackage, label) && parent.clickable
                && parent.parentIndex != index && parent.parentIndex != parentIndex
                && parent.left < parent.right && parent.top < parent.bottom
                && child.left < child.right && child.top < child.bottom
                && parent.left <= child.left && child.right <= parent.right
                && parent.top <= child.top && child.bottom <= parent.bottom) return parentIndex;
        return index;
    }
}
