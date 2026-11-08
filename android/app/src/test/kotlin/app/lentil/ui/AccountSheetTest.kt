package app.lentil.ui

import app.lentil.data.RuleWords
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AccountSheetTest {
    @Test
    fun `switch is hidden without words`() {
        assertFalse(showsSwitch(RuleWords(emptyList(), emptyList())))
        assertTrue(showsSwitch(RuleWords(listOf("eckladen"), emptyList())))
        assertTrue(showsSwitch(RuleWords(emptyList(), listOf("eckladen"))))
    }

    @Test
    fun `the selection keeps each field's order and needs one word`() {
        val words = RuleWords(listOf("zahlfix", "europe"), listOf("streamflix", "abo"))
        assertEquals(
            RuleWords(listOf("zahlfix"), listOf("streamflix")),
            selectedRule(
                words,
                setOf(WordField.Purpose to "streamflix", WordField.Counterparty to "zahlfix"),
            ),
        )
        assertEquals(
            RuleWords(listOf("zahlfix", "europe"), emptyList()),
            selectedRule(
                words,
                setOf(WordField.Counterparty to "europe", WordField.Counterparty to "zahlfix"),
            ),
        )
        assertNull(selectedRule(words, emptySet()))
    }

    @Test
    fun `a word counts only in the field it was selected in`() {
        val words = RuleWords(listOf("streamflix"), listOf("streamflix", "abo"))
        assertEquals(
            RuleWords(emptyList(), listOf("streamflix")),
            selectedRule(words, setOf(WordField.Purpose to "streamflix")),
        )
    }

    private val zahlfix = RuleWords(listOf("zahlfix", "europe"), listOf("streamflix", "abo"))
    private val subscriptions = Pick("Expenses:Subscriptions")

    @Test
    fun `while the switch is off a tap saves at once without a rule`() {
        val gesture = RuleGesture(zahlfix)
        assertTrue(gesture.offered)
        assertFalse(gesture.on)
        assertEquals(Step.Save(subscriptions, null), gesture.choose(subscriptions))
        assertNull(gesture.picked)
    }

    @Test
    fun `without words the sheet offers no switch`() {
        val none = RuleGesture(RuleWords(emptyList(), emptyList()))
        assertFalse(none.offered)
        assertEquals(Step.Save(subscriptions, null), none.choose(subscriptions))
    }

    @Test
    fun `switched on, counterparty words are selected and a tap only picks and previews`() {
        val gesture = RuleGesture(zahlfix)
        gesture.switch(true)

        assertEquals(
            setOf(WordField.Counterparty to "zahlfix", WordField.Counterparty to "europe"),
            gesture.selected,
        )
        assertEquals(zahlfix.purpose, gesture.wordsOf(WordField.Purpose))
        val rule = RuleWords(listOf("zahlfix", "europe"), emptyList())
        assertEquals(Step.Preview(subscriptions, rule), gesture.choose(subscriptions))
        assertEquals(subscriptions, gesture.picked)
        assertEquals(Step.Save(subscriptions, rule), gesture.save())
    }

    @Test
    fun `changing the chips previews again and saves the new selection`() {
        val gesture = RuleGesture(zahlfix)
        gesture.switch(true)
        assertNull(gesture.toggle(WordField.Purpose to "streamflix"))

        val garden = Pick("Expenses:Garden", new = true)
        gesture.choose(garden)
        val purposeOnly = RuleWords(emptyList(), listOf("streamflix"))
        gesture.toggle(WordField.Counterparty to "zahlfix")
        assertEquals(
            Step.Preview(garden, purposeOnly),
            gesture.toggle(WordField.Counterparty to "europe"),
        )
        assertEquals(Step.Save(garden, purposeOnly), gesture.save())
    }

    @Test
    fun `without a selected word nothing previews or saves`() {
        val gesture = RuleGesture(zahlfix)
        gesture.switch(true)
        gesture.toggle(WordField.Counterparty to "zahlfix")
        gesture.toggle(WordField.Counterparty to "europe")

        assertNull(gesture.rule)
        assertNull(gesture.choose(subscriptions))
        assertEquals(subscriptions, gesture.picked)
        assertNull(gesture.save())
    }

    @Test
    fun `nothing saves before an account is picked, and switching off forgets it`() {
        val gesture = RuleGesture(zahlfix)
        gesture.switch(true)
        assertNull(gesture.save())

        gesture.choose(subscriptions)
        gesture.switch(false)
        assertNull(gesture.picked)
        assertNull(gesture.save())
        assertEquals(Step.Save(subscriptions, null), gesture.choose(subscriptions))
    }
}
