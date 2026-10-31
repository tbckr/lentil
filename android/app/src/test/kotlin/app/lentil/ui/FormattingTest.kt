package app.lentil.ui

import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FormattingTest {
    @Test
    fun `amounts keep their decimal places in every locale`() {
        assertEquals("-42.10 EUR", formatAmount("-42.10", "EUR", Locale.US))
        assertEquals("-42,10 EUR", formatAmount("-42.10", "EUR", Locale.GERMANY))
        assertEquals("1.234,5 EUR", formatAmount("1234.5", "EUR", Locale.GERMANY))
        assertEquals("100 EUR", formatAmount("100", "EUR", Locale.US))
    }

    @Test
    fun `unparsable amounts are shown as written`() {
        assertEquals("n/a EUR", formatAmount("n/a", "EUR", Locale.US))
        assertEquals("12.00", formatAmount("12.00", null, Locale.US))
    }

    @Test
    fun `account names need a root type and capitalized components`() {
        assertTrue(isAccountName("Expenses:Garden"))
        assertTrue(isAccountName("Assets:Bank:Giro-2"))
        assertTrue(isAccountName("Expenses:Ärzte"))
        assertFalse(isAccountName("Expenses"))
        assertFalse(isAccountName("expenses:garden"))
        assertFalse(isAccountName("Expenses:garden"))
        assertFalse(isAccountName("Spending:Garden"))
        assertFalse(isAccountName("Expenses:Garden Tools"))
    }
}
