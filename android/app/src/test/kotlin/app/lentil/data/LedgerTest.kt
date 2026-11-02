package app.lentil.data

import org.junit.Assert.assertEquals
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
}
