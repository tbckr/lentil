package app.lentil.data

import java.nio.file.Files
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class IntentStateTest {
    private val a = StoredIntent("a", Intent.Confirm("giro-3f9a1c2e7b4d"), "a")
    private val b = StoredIntent("b", Intent.Dismiss("giro-0a1b2c3d4e5f"), "b")
    private val c = StoredIntent("c", Intent.OpenAccount("Expenses:Garden", "2026-01-02"), "c")

    @Test
    fun `after a sync only intents without outcome stay pending`() {
        val state = IntentState(pending = listOf(a, b, c))

        val next =
            state.afterSync(
                mapOf(
                    "a" to IntentOutcome.Applied,
                    "b" to IntentOutcome.Conflict("TransactionDismissed", null),
                )
            )

        assertEquals(listOf(c), next.pending)
        assertEquals(listOf(StoredConflict(b, "TransactionDismissed")), next.conflicts)
    }

    @Test
    fun `new conflicts join the existing ones`() {
        val old = StoredConflict(c, "AccountNotOpen")
        val state = IntentState(pending = listOf(a), conflicts = listOf(old))

        val next = state.afterSync(mapOf("a" to IntentOutcome.Conflict("AccountChanged", "X:Y")))

        assertEquals(listOf(old, StoredConflict(a, "AccountChanged", "X:Y")), next.conflicts)
    }

    @Test
    fun `discard removes only that conflict`() {
        val state =
            IntentState(
                conflicts =
                    listOf(StoredConflict(a, "NotEditable"), StoredConflict(b, "FileExists"))
            )

        assertEquals(listOf(StoredConflict(b, "FileExists")), state.discard("a").conflicts)
    }

    @Test
    fun `the store writes the spec's operation names and reads them back`() = runTest {
        val directory = Files.createTempDirectory("intents").toFile()
        val file = directory.resolve("intents.json")
        val store = JsonIntentStore(file)
        val set =
            StoredIntent(
                "d",
                Intent.SetAccount(
                    "giro-3f9a1c2e7b4d",
                    1,
                    "Expenses:Uncategorized",
                    "Expenses:Garden",
                ),
                "d",
            )
        val tags =
            listOf(
                StoredIntent("e", Intent.AddTag("giro-3f9a1c2e7b4d", "friends"), "e"),
                StoredIntent("f", Intent.RemoveTag("giro-3f9a1c2e7b4d", "recurring"), "f"),
                StoredIntent(
                    "g",
                    Intent.StartTrip("k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a", "2026-10-30", "lisbon"),
                    "g",
                ),
                StoredIntent(
                    "h",
                    Intent.EndTrip("k7p2q9xd-01jcfh3k5n7p9r1s3t5v7w9y2b", "2026-11-02"),
                    "h",
                ),
            )
        val state =
            IntentState(
                pending = listOf(a, b, c, set) + tags,
                conflicts = listOf(StoredConflict(a, "AccountChanged", "X:Y")),
            )

        store.write(state)

        val json = file.readText()
        listOf(
                "\"op\":\"confirm\"",
                "\"op\":\"dismiss\"",
                "\"op\":\"open-account\"",
                "\"op\":\"set-account\"",
                "\"op\":\"add-tag\"",
                "\"op\":\"remove-tag\"",
                "\"op\":\"start-trip\"",
                "\"op\":\"end-trip\"",
            )
            .forEach { assertTrue("missing $it in $json", json.contains(it)) }
        assertEquals(state, store.read())
        directory.deleteRecursively()
    }

    @Test
    fun `new intents survive a round trip through the store`() = runTest {
        val directory = Files.createTempDirectory("intents").toFile()
        val file = directory.resolve("intents.json")
        val store = JsonIntentStore(file)
        val state =
            IntentState(
                pending =
                    listOf(
                        StoredIntent(
                            "i",
                            Intent.SuggestAccount(
                                "giro-1",
                                1,
                                "Expenses:Uncategorized",
                                "Expenses:Food",
                            ),
                            "i",
                        ),
                        StoredIntent(
                            "j",
                            Intent.AddRule(
                                purpose = "(?i)(?:^|\\PL)streamflix(?:\\PL|$)",
                                account = "Expenses:Subscriptions",
                            ),
                            "j",
                        ),
                    )
            )

        store.write(state)

        val json = file.readText()
        listOf("\"op\":\"suggest-account\"", "\"op\":\"add-rule\"").forEach {
            assertTrue("missing $it in $json", json.contains(it))
        }
        assertEquals(state, store.read())
        directory.deleteRecursively()
    }

    @Test
    fun `a missing file is an empty state`() = runTest {
        val directory = Files.createTempDirectory("intents").toFile()

        assertEquals(IntentState(), JsonIntentStore(directory.resolve("intents.json")).read())
        directory.deleteRecursively()
    }
}
