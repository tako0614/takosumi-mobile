package com.takosumi.fixture.webhome;

/** A software-keyboard Save wait observes its IME, one Back, then absence. */
final class ConnectionSaveKeyboard {
    interface Probe {
        boolean isShown();
        boolean dismiss();
    }

    private boolean backRequested;

    boolean ready(Probe probe) {
        // Input can return before its requested software IME appears. An
        // initial empty query must not authorize Save under that pending show.
        if (!probe.isShown()) return backRequested;
        if (!backRequested) {
            // A false return (or throw) does not authorize another Back.
            backRequested = true;
            probe.dismiss();
        }
        // Reacquire Save after dismissal, even if Back hid the IME immediately.
        return false;
    }
}
