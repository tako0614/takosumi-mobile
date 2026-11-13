package com.takosumi.appscreen

import java.util.ArrayDeque
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit
import org.junit.Assert.*
import org.junit.Test

class AppScreenCookiePersistenceTest {
    private class Fixture {
        val ui = Thread.currentThread()
        val jobs = ArrayDeque<Runnable>()
        val callbacks = ArrayDeque<() -> Unit>()
        val generations = mutableMapOf("work" to 0L, "personal" to 0L)
        val receipts = mutableListOf<Pair<String, AppScreenCookiePersistence.Outcome>>()
        var stopped = false
        val queue = AppScreenCookiePersistence(
            worker = { job -> if (stopped) throw RejectedExecutionException() else jobs.add(job) },
            post = { callbacks.add(it) },
            checkUi = { check(Thread.currentThread() === ui) },
            generation = { generations[it] ?: 0L },
            receipt = { owner, outcome -> receipts.add(owner.profile to outcome) },
        )
        fun owner(profile: String, at: Long = generations[profile] ?: 0L, flush: () -> Unit) =
            AppScreenCookiePersistence.Owner(profile, at, flush)
        fun complete() { jobs.remove().run(); callbacks.remove().invoke() }
    }

    @Test fun requestsAreCoalescedButALaterDirtyRequestIsNotLost() {
        val f = Fixture()
        var work = 0
        var personal = 0
        val w = f.owner("work") { work++ }
        val p = f.owner("personal") { personal++ }
        assertTrue(f.queue.request(w))
        repeat(100) { assertTrue(f.queue.request(w)) }
        assertTrue(f.queue.request(p))
        assertEquals(2, f.jobs.size)
        f.complete() // schedules exactly one later Work flush, behind Personal.
        assertEquals(2, f.jobs.size)
        f.complete()
        f.complete()
        assertEquals(2, work)
        assertEquals(1, personal)
        assertTrue(f.jobs.isEmpty())
        assertTrue(f.callbacks.isEmpty())
        assertEquals(3, f.receipts.count { it.second == AppScreenCookiePersistence.Outcome.FLUSHED })
    }

    @Test fun clearWaitsForTheOwnedFlushAndRefusesLateRequestsAndOldGeneration() {
        val f = Fixture()
        var flushes = 0
        val old = f.owner("work") { flushes++ }
        f.queue.request(old)
        f.queue.request(old)
        var lease: AppScreenCookiePersistence.Clear? = null
        assertTrue(f.queue.beginClear("work", 0) { lease = it })
        assertNull(lease)
        assertFalse(f.queue.request(old))
        assertFalse(f.queue.beginClear("work", 0) { fail("duplicate clear") })
        f.complete()
        assertNotNull(lease)
        assertEquals(1, flushes)
        assertTrue(f.jobs.isEmpty())
        assertFalse(f.receipts.any { it.second == AppScreenCookiePersistence.Outcome.FLUSHED })
        f.generations["work"] = 1
        f.queue.finishClear(requireNotNull(lease))
        assertFalse(f.queue.request(old))
        assertTrue(f.queue.request(f.owner("work") { flushes++ }))
        f.complete()
        // An old clear callback cannot remove a newly running state's owner.
        f.queue.request(f.owner("work") { flushes++ })
        f.queue.finishClear(requireNotNull(lease))
        f.complete()
        assertEquals(3, flushes)
        assertEquals(2, f.receipts.count { it.second == AppScreenCookiePersistence.Outcome.FLUSHED })
    }

    @Test fun aFailedFlushIsNotSuccessAndDoesNotStrandClearOrOtherProfiles() {
        val f = Fixture()
        f.queue.request(f.owner("work") { throw IllegalStateException("synthetic failure") })
        var clearFinished = false
        assertTrue(f.queue.beginClear("work", 0) { lease ->
            clearFinished = true
            f.queue.finishClear(lease)
        })
        var personal = 0
        f.queue.request(f.owner("personal") { personal++ })
        f.complete()
        f.complete()
        assertTrue(clearFinished)
        assertEquals(1, personal)
        assertTrue(f.receipts.contains("work" to AppScreenCookiePersistence.Outcome.FAILED))
        assertFalse(f.receipts.contains("work" to AppScreenCookiePersistence.Outcome.FLUSHED))
        assertTrue(f.jobs.isEmpty())
    }

    @Test fun stoppedWorkerCannotQualifyPersistenceAndRejectedStateCanBeCleared() {
        val f = Fixture()
        f.stopped = true
        var calls = 0
        assertFalse(f.queue.request(f.owner("work") { calls++ }))
        assertEquals(0, calls)
        assertEquals(listOf("work" to AppScreenCookiePersistence.Outcome.FAILED), f.receipts)
        var cleared = false
        assertTrue(f.queue.beginClear("work", 0) { lease -> cleared = true; f.queue.finishClear(lease) })
        assertTrue(cleared)
        assertTrue(f.jobs.isEmpty())
        assertTrue(f.callbacks.isEmpty())
    }

    @Test fun aGenerationChangeBeforeCompletionCannotQualifyTheNewGeneration() {
        val f = Fixture()
        var calls = 0
        f.queue.request(f.owner("work") { calls++ })
        f.queue.request(f.owner("work") { calls++ })
        f.jobs.remove().run()
        f.generations["work"] = 1
        f.callbacks.remove().invoke()
        assertEquals(1, calls)
        assertEquals(listOf("work" to AppScreenCookiePersistence.Outcome.RETIRED), f.receipts)
        assertTrue(f.jobs.isEmpty())
    }

    @Test fun pendingProfilesAreBoundedEvenIfTheWorkerDoesNotProgress() {
        val f = Fixture()
        repeat(AppScreenCookiePersistence.MAX_PROFILES) { n ->
            assertTrue(f.queue.request(f.owner("profile-$n") {}))
        }
        assertFalse(f.queue.request(f.owner("overflow") { fail("overflow operation") }))
        assertFalse(f.queue.beginClear("overflow", 0) { fail("overflow clear") })
        assertEquals(AppScreenCookiePersistence.MAX_PROFILES, f.jobs.size)
        repeat(AppScreenCookiePersistence.MAX_PROFILES) { f.complete() }
        assertTrue(f.jobs.isEmpty())
        assertTrue(f.callbacks.isEmpty())
    }

    @Test fun unreadableGenerationRefusesPersistenceAndClearWithoutRunningTheJar() {
        val f = Fixture()
        f.generations["work"] = -1
        assertFalse(f.queue.request(f.owner("work") { fail("unknown generation flush") }))
        assertFalse(f.queue.beginClear("work", -1) { fail("unknown generation clear") })
        assertEquals(listOf("work" to AppScreenCookiePersistence.Outcome.FAILED), f.receipts)
        assertTrue(f.jobs.isEmpty())
        assertTrue(f.callbacks.isEmpty())
    }

    @Test fun blockingFlushRunsOffUiAndClearDoesNotPretendItHasCompleted() {
        val ui = Thread.currentThread()
        val callbacks = java.util.concurrent.LinkedBlockingQueue<() -> Unit>()
        val executor = Executors.newSingleThreadExecutor()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val outcomes = mutableListOf<AppScreenCookiePersistence.Outcome>()
        val queue = AppScreenCookiePersistence(executor, { callbacks.add(it) },
            { check(Thread.currentThread() === ui) }, { 0L }, { _, outcome -> outcomes.add(outcome) })
        try {
            queue.request(AppScreenCookiePersistence.Owner("personal", 0) {
                assertNotSame(ui, Thread.currentThread())
                entered.countDown()
                check(release.await(5, TimeUnit.SECONDS))
            })
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            var clear: AppScreenCookiePersistence.Clear? = null
            assertTrue(queue.beginClear("personal", 0) { clear = it })
            assertNull(clear)
            assertTrue(outcomes.isEmpty())
            release.countDown()
            requireNotNull(callbacks.poll(5, TimeUnit.SECONDS)).invoke()
            assertNotNull(clear)
            assertEquals(listOf(AppScreenCookiePersistence.Outcome.RETIRED), outcomes)
            queue.finishClear(requireNotNull(clear))
            assertTrue(callbacks.isEmpty())
        } finally {
            release.countDown()
            executor.shutdownNow()
            assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS))
        }
    }

    @Test fun interruptedWorkerDoesNotReportSuccessOrLeaveAClearBarrierStuck() {
        val ui = Thread.currentThread()
        val callbacks = java.util.concurrent.LinkedBlockingQueue<() -> Unit>()
        val executor = Executors.newSingleThreadExecutor()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val outcomes = mutableListOf<AppScreenCookiePersistence.Outcome>()
        val queue = AppScreenCookiePersistence(executor, { callbacks.add(it) },
            { check(Thread.currentThread() === ui) }, { 0L }, { _, outcome -> outcomes.add(outcome) })
        try {
            assertTrue(queue.request(AppScreenCookiePersistence.Owner("work", 0) {
                entered.countDown()
                release.await()
            }))
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            var cleared = false
            queue.beginClear("work", 0) { lease -> cleared = true; queue.finishClear(lease) }
            assertFalse(cleared)
            executor.shutdownNow()
            assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS))
            requireNotNull(callbacks.poll(5, TimeUnit.SECONDS)).invoke()
            assertTrue(cleared)
            assertEquals(listOf(AppScreenCookiePersistence.Outcome.FAILED), outcomes)
            assertTrue(callbacks.isEmpty())
        } finally {
            release.countDown()
            executor.shutdownNow()
            assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS))
        }
    }
}
