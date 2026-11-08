package app.lentil.data

import app.lentil.ui.entry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LedgerTest {
    @Test
    fun `a counter posting may move only to accounts open on the transaction's date`() {
        val accounts =
            listOf(
                LedgerAccount("Assets:Bank:Giro", null),
                LedgerAccount("Expenses:Garden", "2026-10-18"),
                LedgerAccount("Expenses:Groceries", "2026-01-01"),
                LedgerAccount("Expenses:Travel", null),
            )

        assertEquals(listOf("Expenses:Groceries"), accounts.usableOn("2026-10-17"))
        assertEquals(
            listOf("Expenses:Garden", "Expenses:Groceries"),
            accounts.usableOn("2026-10-18"),
        )
    }

    @Test
    fun `a queue entry carries no tags, suggestion or rule words unless the core gives them`() {
        val plain = entry("giro-3f9a1c2e7b4d")
        assertEquals(emptyList<String>(), plain.tags)
        assertNull(plain.suggestedTag)
        assertTrue(plain.ruleWords.isEmpty)

        val tagged = entry("giro-3f9a1c2e7b4d", tags = listOf("friends"), suggested = "trip-lisbon")
        assertEquals(listOf("friends"), tagged.tags)
        assertEquals("trip-lisbon", tagged.suggestedTag)

        assertNull(LedgerView(emptyList(), emptyList(), null, emptyList()).runningTrip)
    }
}
