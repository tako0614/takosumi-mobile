package com.takosumi.fixture.geckodownload;

import java.io.*;
import java.util.*;

/** Runs the actual fixture I/O helper with owned JDK streams, not SDK stubs. */
public final class ExportIoOperationTest {
    static int checks;
    interface Action { void run() throws Exception; }
    static void check(boolean value) { checks++; if (!value) throw new AssertionError("export I/O control"); }
    static void equal(Object actual, Object expected) { check(Objects.equals(actual, expected)); }
    static Exception failure(Action action) throws Exception {
        try { action.run(); } catch (Exception e) { return e; }
        throw new AssertionError("Expected original failure");
    }

    static void distinguishIdenticalLegacyErrors() throws Exception {
        IOException readError = new IOException("Timed out"), writeError = new IOException("Timed out");
        ExportIoOperation read = new ExportIoOperation(), write = new ExportIoOperation();
        int[] calls = {0, 0};
        InputStream input = new InputStream() {
            public int read() { throw new AssertionError("Unexpected scalar read"); }
            public int read(byte[] buffer) throws IOException { calls[0]++; throw readError; }
        };
        OutputStream output = new OutputStream() {
            public void write(int value) { throw new AssertionError("Unexpected scalar write"); }
            public void write(byte[] buffer, int offset, int count) throws IOException { calls[1]++; throw writeError; }
        };
        Exception caughtRead = failure(() -> read.read(input, new byte[1]));
        Exception caughtWrite = failure(() -> write.write(output, new byte[1], 0, 1));
        check(caughtRead == readError); check(caughtWrite == writeError);
        equal(read.error(caughtRead, 0), "IOException: Timed out after 0B");
        equal(write.error(caughtWrite, 0), read.error(caughtRead, 0));
        read.observeFailure(caughtRead, input); write.observeFailure(caughtWrite, input);
        equal(read.failedOperation(), "read"); equal(write.failedOperation(), "write");
        check(!read.failedOperation().equals(write.failedOperation()));
        check(Arrays.equals(calls, new int[] {1, 1}));
    }

    static void normalBytesAndClosure() throws Exception {
        ExportIoOperation operation = new ExportIoOperation();
        List<String> order = new ArrayList<>();
        int[] opened = {0}, reads = {0}, writes = {0};
        ByteArrayInputStream input = new ByteArrayInputStream(new byte[] {3, 5, 7}) {
            public int read(byte[] buffer) throws IOException { reads[0]++; return super.read(buffer); }
            public void close() { order.add("input"); }
        };
        ByteArrayOutputStream output = new ByteArrayOutputStream() {
            public void write(byte[] buffer, int offset, int count) { writes[0]++; super.write(buffer, offset, count); }
            public void close() { order.add("output"); }
        };
        try (InputStream in = input; OutputStream out = operation.openOutput(() -> { opened[0]++; return output; })) {
            operation.beforeValidation();
            byte[] data = new byte[4];
            int count = operation.read(in, data);
            equal(count, 3);
            operation.write(out, data, 0, count);
            equal(operation.read(in, data), -1);
            operation.beforeResourceClose();
        }
        operation.closeOriginalFinally(input);
        check(Arrays.equals(output.toByteArray(), new byte[] {3, 5, 7}));
        check(Arrays.equals(new int[] {opened[0], reads[0], writes[0]}, new int[] {1, 2, 1}));
        equal(order, List.of("output", "input", "input"));
        equal(operation.failedOperation(), "unknown");
    }

    static void originalPrimaryAndSuppressed() throws Exception {
        ExportIoOperation operation = new ExportIoOperation();
        List<String> order = new ArrayList<>();
        IOException primary = new IOException("original-read"), outputClose = new IOException("output-close"), inputClose = new IOException("input-close");
        InputStream input = new InputStream() {
            public int read() { throw new AssertionError("Unexpected scalar read"); }
            public int read(byte[] data) throws IOException { throw primary; }
            public void close() throws IOException { order.add("input"); throw inputClose; }
        };
        OutputStream output = new OutputStream() {
            public void write(int value) { throw new AssertionError("Unexpected write"); }
            public void close() throws IOException { order.add("output"); throw outputClose; }
        };
        Exception caught = failure(() -> {
            try (InputStream in = input; OutputStream out = operation.openOutput(() -> output)) {
                operation.read(in, new byte[1]);
                operation.beforeResourceClose();
            }
        });
        check(caught == primary);
        check(Arrays.equals(caught.getSuppressed(), new Throwable[] {outputClose, inputClose}));
        equal(order, List.of("output", "input"));
        operation.observeFailure(caught, input);
        equal(operation.failedOperation(), "read");
    }

    static void openValidationAndResourceClose() throws Exception {
        ExportIoOperation open = new ExportIoOperation();
        IOException expected = new IOException("open-private-sentinel");
        Exception caught = failure(() -> open.openOutput(() -> { throw expected; }));
        check(caught == expected); open.observeFailure(caught, null);
        equal(open.failedOperation(), "open_output"); equal(open.bodyKind(), "none");
        equal(open.origin(), "unclassified");
        ExportIoOperation validation = new ExportIoOperation();
        check(validation.openOutput(() -> null) == null);
        validation.beforeValidation(); validation.observeFailure(new IOException("missing"), null);
        equal(validation.failedOperation(), "validate");
        ExportIoOperation closing = new ExportIoOperation();
        List<String> order = new ArrayList<>();
        IOException close = new IOException("close"), suppressed = new IOException("input-close");
        InputStream input = new ByteArrayInputStream(new byte[0]) {
            public void close() throws IOException { order.add("input"); throw suppressed; }
        };
        OutputStream output = new ByteArrayOutputStream() {
            public void close() throws IOException { order.add("output"); throw close; }
        };
        caught = failure(() -> {
            try (InputStream in = input; OutputStream out = closing.openOutput(() -> output)) {
                closing.beforeResourceClose();
            }
        });
        check(caught == close); check(Arrays.equals(caught.getSuppressed(), new Throwable[] {suppressed}));
        closing.observeFailure(caught, input);
        equal(closing.failedOperation(), "resource_close"); equal(order, List.of("output", "input"));
    }

    static void finalCloseAndRuntimeIdentity() throws Exception {
        ExportIoOperation operation = new ExportIoOperation();
        IOException close = new IOException("final-close");
        InputStream body = new ByteArrayInputStream(new byte[0]) {
            public void close() throws IOException { throw close; }
        };
        Exception caught = failure(() -> operation.closeOriginalFinally(body));
        check(caught == close); operation.observeFailure(caught, body);
        equal(operation.failedOperation(), "final_original_close");
        RuntimeException runtime = new IllegalStateException("runtime");
        check(failure(() -> operation.openOutput(() -> { throw runtime; })) == runtime);
        equal(operation.error(runtime, 17), "IllegalStateException: runtime after 17B");
        InputStream runtimeClose = new ByteArrayInputStream(new byte[0]) {
            public void close() { throw runtime; }
        };
        check(failure(() -> operation.closeOriginalFinally(runtimeClose)) == runtime);
    }

    static void closedMetadataAndPoison() throws Exception {
        equal(ExportIoOperation.projectBodyKind("org.mozilla.geckoview.GeckoInputStream"), "gecko_input");
        equal(ExportIoOperation.projectBodyKind("private-unmatched-class"), "other");
        equal(ExportIoOperation.projectBodyKind(null), "none");
        // These are synthetic metadata tuples, not a fake SDK class/runtime.
        StackTraceElement read = new StackTraceElement("org.mozilla.geckoview.GeckoInputStream", "read", "ignored", 55);
        equal(ExportIoOperation.projectOrigin("read", "gecko_input", true, read), "gecko_input_read");
        equal(ExportIoOperation.projectOrigin("write", "gecko_input", true, read), "unclassified");
        equal(ExportIoOperation.projectOrigin("read", "other", true, read), "unclassified");
        equal(ExportIoOperation.projectOrigin("read", "gecko_input", false, read), "unclassified");
        equal(ExportIoOperation.projectOrigin("read", "gecko_input", true, null), "unavailable");
        equal(ExportIoOperation.projectOrigin("read", "gecko_input", true,
            new StackTraceElement("private-frame", "private-method", "private-file", 1)), "unclassified");
        ExportIoOperation operation = new ExportIoOperation();
        int[] poisoned = {0};
        IOException custom = new IOException("private-message") {
            public StackTraceElement[] getStackTrace() { poisoned[0]++; throw new AssertionError("metadata poison"); }
        };
        equal(ExportIoOperation.classifyException("read", "gecko_input", custom), "unclassified");
        equal(poisoned[0], 0);
        IOException synthetic = new IOException("private-message");
        synthetic.setStackTrace(new StackTraceElement[] {read});
        equal(ExportIoOperation.classifyException("read", "gecko_input", synthetic), "gecko_input_read");
        synthetic.setStackTrace(new StackTraceElement[0]);
        equal(ExportIoOperation.classifyException("read", "gecko_input", synthetic), "unavailable");
        operation.read(new ByteArrayInputStream(new byte[] {1}), new byte[1]);
        operation.observeFailure(custom, new ByteArrayInputStream(new byte[0]));
        equal(poisoned[0], 0); equal(operation.failedOperation(), "read"); equal(operation.origin(), "unclassified");
        check(!operation.failedOperation().contains("private")); check(!operation.bodyKind().contains("private"));
        check(!operation.origin().contains("private"));
        operation.observeFailure(null, null);
        equal(operation.origin(), "unavailable");
    }

    public static void main(String[] args) throws Exception {
        distinguishIdenticalLegacyErrors(); normalBytesAndClosure(); originalPrimaryAndSuppressed();
        openValidationAndResourceClose(); finalCloseAndRuntimeIdentity(); closedMetadataAndPoison();
        System.out.println("PASS: original export I/O operation checks=" + checks);
    }
}
