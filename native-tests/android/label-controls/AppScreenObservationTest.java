package com.takosumi.appscreen;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

/** Existing-JDK primitive controls only; never loads Android or the App. */
public final class AppScreenObservationTest {
    private static int checks;
    private static void check(boolean value) {
        checks++;
        if (!value) throw new AssertionError("native observation control " + checks);
    }
    private static void emit(AppScreenObservation value) {
        value.record(AppScreenObservation.Stage.BACK_ENTER, 99, true, true, 0,
                     true, true, false, AppScreenObservation.Logical.UNKNOWN);
    }
    public static void main(String[] args) {
        AtomicInteger clocks = new AtomicInteger();
        List<String> output = new ArrayList<>();
        AppScreenObservation release = new AppScreenObservation(false, 7,
            () -> { clocks.incrementAndGet(); throw new AssertionError(); }, output::add);
        for (int i = 0; i < 100; i++) emit(release);
        check(clocks.get() == 0);
        check(output.isEmpty());
        AppScreenObservation debug = new AppScreenObservation(true, 7,
            () -> 10 + clocks.incrementAndGet(), output::add);
        for (int i = 0; i < 100; i++) emit(debug);
        check(output.size() == 64);
        check(clocks.get() == 64);
        check(output.get(0).equals("v=1 seq=1 pid=7 t=11 stage=BACK_ENTER view=99 cur=1 host=1 gen=0 vis=1 res=1 evict=0 logical=UNKNOWN physical=unknown cap=0"));
        check(output.get(63).contains("seq=64") && output.get(63).contains("stage=CAP_EXHAUSTED")
              && output.get(63).endsWith("physical=unknown cap=1"));
        check(output.stream().filter(line -> line.contains("cap=1")).count() == 1);
        check(output.stream().allMatch(line -> line.length() <= 320 && !line.contains("\n") && !line.contains("\r")));
        AtomicInteger failures = new AtomicInteger();
        AppScreenObservation badSink = new AppScreenObservation(true, 7, () -> 12,
            line -> { failures.incrementAndGet(); throw new AssertionError("private detail"); });
        for (int i = 0; i < 100; i++) emit(badSink);
        check(failures.get() == 64);
        AtomicInteger clockFailures = new AtomicInteger();
        AppScreenObservation badClock = new AppScreenObservation(true, 7,
            () -> { clockFailures.incrementAndGet(); throw new AssertionError("private clock"); },
            line -> { throw new AssertionError("must not log"); });
        for (int i = 0; i < 100; i++) emit(badClock);
        check(clockFailures.get() == 64);
        List<String> maximum = new ArrayList<>();
        AppScreenObservation edge = new AppScreenObservation(true, Integer.MAX_VALUE, () -> Long.MAX_VALUE, maximum::add);
        edge.record(AppScreenObservation.Stage.BIND_RETURN, Integer.MIN_VALUE, false, false,
                    Long.MIN_VALUE, false, false, true, AppScreenObservation.Logical.REMOVE_RETURN);
        check(maximum.size() == 1 && maximum.get(0).length() <= 320);
        RuntimeException original = new RuntimeException("functional sentinel");
        Throwable caught = null;
        try { try { throw original; } finally { emit(badSink); } } catch (Throwable error) { caught = error; }
        check(caught == original);
        System.out.println("PASS: native observation controls checks=" + checks);
    }
}
