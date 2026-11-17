package com.takosumi.fixture.webhome;

import java.io.IOException;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Runs the production metadata helper only; no Android/AX or fake SDK types. */
public final class PersonalHomeWaitObservationTest {
    private static int checks;
    private static void check(boolean ok) { checks++; if (!ok) throw new AssertionError("personal wait " + checks); }
    private static final BackWaitObservation.Outcome NULL = BackWaitObservation.Outcome.QUERY_NULL;
    private static final BackWaitObservation.Outcome VISIBLE = BackWaitObservation.Outcome.VISIBLE;
    private static PersonalHomeWaitObservation value() {
        PersonalHomeWaitObservation value = new PersonalHomeWaitObservation();
        value.begin(100, 15100);
        return value;
    }
    private static PersonalHomeWaitObservation.Capture capture(long at) {
        return new PersonalHomeWaitObservation.Capture(at, at + 1, new PersonalHomeWaitObservation.Window[] {
            new PersonalHomeWaitObservation.Window(7, 7, 1, true, true, true),
            new PersonalHomeWaitObservation.Window(9, -1, 99, false, false, false)
        });
    }
    public static void main(String[] args) {
        PersonalHomeWaitObservation value = value();
        int[] queries = {0};
        value.sample(NULL, 200, () -> { queries[0]++; return capture(200); });
        value.sample(NULL, 201, () -> { throw new AssertionError("unchanged enum must not query context"); });
        value.sample(BackWaitObservation.Outcome.DISABLED, 202, () -> capture(202));
        value.sample(BackWaitObservation.Outcome.EMPTY_BOUNDS, 203, () -> capture(203));
        value.sample(VISIBLE, 204, () -> capture(204));
        check(queries[0] == 1 && value.samples == 5 && value.queryNull == 2 && value.disabled == 1
            && value.emptyBounds == 1 && value.visible == 1);
        check(value.first == NULL && value.last == VISIBLE && value.contexts.size() == 4);
        Map<String, Object> context = value.contexts.get(0);
        check(context.get("querySelectedRoot").equals("unknown"));
        check(context.get("relation").equals("post_query_window_context"));
        List<?> windows = (List<?>) context.get("windows");
        Map<?, ?> known = (Map<?, ?>) windows.get(0), absent = (Map<?, ?>) windows.get(1);
        check(known.get("windowId").equals(7) && known.get("rootWindowId").equals(7)
            && known.get("rootWindowAgrees").equals(true));
        check(absent.containsKey("rootWindowId") && absent.get("rootWindowId") == null
            && absent.get("rootWindowAgrees") == null && absent.get("rootPackageMatchesTarget") == null
            && absent.get("type").equals(0));
        value.finish(205, true);
        value.sample(NULL, 206, () -> { throw new AssertionError("finished scope"); });
        check(value.samples == 5 && value.returned && !value.waiting);
        Map<String, Object> metadata = new LinkedHashMap<>();
        check(value.writeTo(metadata::put, false));
        check(metadata.get("phase").equals("post_reset_personal_home")
            && metadata.get("recordingIncomplete").equals(false));
        check(metadata.get("waitStartedAtMs").equals(100L) && metadata.get("waitDeadlineAtMs").equals(15100L));

        PersonalHomeWaitObservation steady = value();
        for (long at : new long[] {100, 200, 5100, 5200, 10100, 14100, 15000}) {
            final long capturedAt = at;
            steady.sample(NULL, at, () -> capture(capturedAt));
        }
        check(steady.samples == 7 && steady.contexts.size() == 4);
        check(steady.contexts.get(0).get("queryReturnedAtMs").equals(100L)
            && steady.contexts.get(3).get("queryReturnedAtMs").equals(14100L));
        check(!steady.contextCapped);

        PersonalHomeWaitObservation missing = value();
        missing.sample(NULL, 200, () -> null);
        check(missing.queryNull == 1 && missing.incomplete
            && missing.contexts.get(0).get("state").equals("unknown"));
        PersonalHomeWaitObservation poison = value();
        poison.sample(NULL, 200, () -> { throw new IOException("SECRET URL body"); });
        check(poison.queryNull == 1 && poison.incomplete
            && poison.contexts.get(0).get("state").equals("unavailable"));
        check(!poison.contexts.toString().contains("SECRET"));
        PersonalHomeWaitObservation late = value();
        late.sample(NULL, 15101, () -> { throw new AssertionError("late getter must not run"); });
        check(late.samples == 0 && late.late == 1 && late.incomplete && late.contexts.isEmpty());
        late.sample(NULL, 200, () -> new PersonalHomeWaitObservation.Capture(200, 15101,
            new PersonalHomeWaitObservation.Window[] {new PersonalHomeWaitObservation.Window(77, 77, 1, true, true, true)}));
        check(late.samples == 1 && late.contexts.get(0).get("state").equals("outside_deadline")
            && ((List<?>) late.contexts.get(0).get("windows")).isEmpty());

        PersonalHomeWaitObservation capped = value();
        for (int i = 0; i < 1000; i++) {
            final long at = 200 + i;
            capped.sample(i % 2 == 0 ? NULL : VISIBLE, at, () -> capture(at));
        }
        check(capped.samples == 256 && capped.queryNull == 128 && capped.visible == 128
            && capped.capped && capped.dropped == 256);
        check(capped.contexts.size() == 4 && capped.contextCapped && capped.contextsDropped == 252);
        check(capped.last == VISIBLE);
        PersonalHomeWaitObservation cut = value();
        cut.sample(NULL, 200, () -> new PersonalHomeWaitObservation.Capture(200, 201,
            new PersonalHomeWaitObservation.Window[] {
                new PersonalHomeWaitObservation.Window(1, 2, 1, false, false, true),
                new PersonalHomeWaitObservation.Window(3, 3, 1, false, false, false),
                new PersonalHomeWaitObservation.Window(4, 4, 1, false, false, false),
                new PersonalHomeWaitObservation.Window(5, 5, 1, false, false, false), null}));
        check(cut.incomplete && cut.contexts.get(0).get("truncated").equals(true));
        List<?> rows = (List<?>) cut.contexts.get(0).get("windows");
        check(rows.size() == 4 && ((Map<?, ?>) rows.get(0)).get("rootWindowAgrees").equals(false));
        PersonalHomeWaitObservation invalid = value();
        invalid.sample(null, 200, () -> { throw new AssertionError("invalid enum"); });
        invalid.sample(NULL, 99, () -> { throw new AssertionError("time before original wait"); });
        check(invalid.incomplete && invalid.samples == 0);
        invalid.sample(NULL, 200, () -> new PersonalHomeWaitObservation.Capture(200, 201,
            new PersonalHomeWaitObservation.Window[] {new PersonalHomeWaitObservation.Window(-1, 0, 1, false, false, false)}));
        check(invalid.contexts.get(0).get("state").equals("partial"));

        AssertionError original = new AssertionError("original wait failure");
        AssertionError observed = null;
        int[] cleanup = {0};
        try {
            try { throw original; }
            finally {
                poison.sample(VISIBLE, 300, () -> { throw new IllegalStateException("SECRET"); });
                poison.finish(301, false);
                check(!poison.writeTo((k, v) -> { throw new IOException("SECRET"); }, false));
                cleanup[0]++;
            }
        } catch (AssertionError error) { observed = error; }
        check(observed == original && cleanup[0] == 1 && !poison.returned);
        Map<String, Object> poisoned = new LinkedHashMap<>();
        check(poison.writeTo(poisoned::put, false) && poisoned.get("recordingIncomplete").equals(true));
        check(!poisoned.toString().contains("SECRET"));
        System.out.println("PASS: personal home wait metadata checks=" + checks);
    }
}
