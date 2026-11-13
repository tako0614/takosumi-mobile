package com.takosumi.fixture.webhome;

/** One Save wait owns at most one Back and waits for observed IME absence. */
final class ConnectionSaveKeyboard {
    interface Probe {
        boolean isShown();
        boolean dismiss();
    }

    private boolean backRequested;

    boolean ready(Probe probe) {
        if (!probe.isShown()) return true;
        if (!backRequested) {
            // A false return (or throw) does not authorize another Back.
            backRequested = true;
            probe.dismiss();
        }
        // Reacquire Save after dismissal, even if Back hid the IME immediately.
        return false;
    }
}
