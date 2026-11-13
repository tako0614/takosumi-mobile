package com.takosumi.fixture.webhome;

public final class ConnectionSaveKeyboardTest {
    private static int checks;

    private static void check(boolean value, String what) {
        checks++;
        if (!value) throw new AssertionError(what);
    }

    private static final class Probe implements ConnectionSaveKeyboard.Probe {
        boolean shown, immediate, backResult = true;
        int reads, backs;
        public boolean isShown() { reads++; return shown; }
        public boolean dismiss() {
            backs++;
            if (immediate) shown = false;
            return backResult;
        }
    }

    public static void main(String[] args) {
        ConnectionSaveKeyboard gate = new ConnectionSaveKeyboard();
        Probe probe = new Probe();
        check(gate.ready(probe), "Hidden before finding Save");
        probe.shown = true; // The selector/bounds read lets a late IME appear.
        check(!gate.ready(probe), "A newly covered Save must be discarded");
        check(probe.backs == 1, "One dismissal request");
        check(!gate.ready(probe), "Back returning does not prove IME disappearance");
        check(probe.backs == 1, "Settling must not send another Back");
        probe.shown = false;
        check(gate.ready(probe), "Save may be reacquired after disappearance");
        check(gate.ready(probe) && probe.backs == 1, "Post-selector read still needs no Back");

        gate = new ConnectionSaveKeyboard();
        probe = new Probe();
        probe.shown = true;
        probe.immediate = true;
        check(!gate.ready(probe), "Immediate dismissal still discards the old control");
        check(gate.ready(probe) && probe.backs == 1, "Fresh readiness after immediate dismissal");
        probe.shown = true;
        check(!gate.ready(probe) && probe.backs == 1, "A reappearing IME cannot authorize a second Back");

        gate = new ConnectionSaveKeyboard();
        probe = new Probe();
        probe.shown = true;
        probe.backResult = false;
        check(!gate.ready(probe), "False Back result is not readiness");
        check(!gate.ready(probe) && probe.backs == 1, "False Back is still consumed once");
        probe.shown = false;
        check(gate.ready(probe), "Actual disappearance determines readiness");

        gate = new ConnectionSaveKeyboard();
        probe = new Probe();
        check(gate.ready(probe) && gate.ready(probe), "No keyboard needs no dismissal");
        check(probe.backs == 0 && probe.reads == 2, "Two readiness reads, no action");
        System.out.println("PASS: connection Save keyboard checks=" + checks);
    }
}
