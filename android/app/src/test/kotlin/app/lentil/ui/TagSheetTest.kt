package app.lentil.ui

import app.lentil.R
import app.lentil.data.TagSource
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TagSheetTest {
    private val plain = { name: String -> name.matches(Regex("[a-z0-9-]+")) }

    @Test
    fun `familyTag joins a plain rest and refuses others`() {
        assertEquals("share-max", familyTag("share-", "max", plain))
        for (rest in listOf("", "Max", "a b", "ä")) {
            assertNull(rest, familyTag("share-", rest, plain))
        }
    }

    @Test
    fun `only choices outside the taxonomy get a hint`() {
        assertNull(sourceHint(TagSource.Taxonomy))
        assertEquals(R.string.tag_source_missing, sourceHint(TagSource.Missing))
        assertEquals(R.string.tag_source_invalid, sourceHint(TagSource.Invalid))
    }

    @Test
    fun `a name shows an error only when it breaks the core's rule`() {
        assertEquals(false, breaksNameRule("", plain))
        assertEquals(false, breaksNameRule("lisbon-2026", plain))
        for (name in listOf("Lisbon", "a b", "ä")) {
            assertEquals(name, true, breaksNameRule(name, plain))
        }
    }
}
