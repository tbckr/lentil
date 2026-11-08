@file:OptIn(ExperimentalMaterial3Api::class)

package app.lentil.ui

import androidx.annotation.StringRes
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import app.lentil.R
import app.lentil.data.LedgerAccount
import app.lentil.data.RuleWords
import app.lentil.data.usableOn

/**
 * Picks a counter account for a transaction on [date]: search, recently used first, or a new
 * account (PRD: FR-D3). It offers only accounts open on [date], because changing the account also
 * confirms the transaction, and a change the core rejects would confirm it on the old account.
 *
 * Above the search, the switch "Always book like this" turns the choice into a rule on the [words]
 * the user selects (PRD: FR-D14), as [RuleGesture] describes; [onPreview] asks for the [preview],
 * the number of other open transactions the rule would move. The [words] come with the queue entry,
 * so the switch is there when the sheet opens and never shifts the list under a tap.
 */
@Composable
fun AccountSheet(
    accounts: List<LedgerAccount>,
    date: String,
    recent: List<String>,
    words: RuleWords,
    preview: Int?,
    onSelect: (account: String, rule: RuleWords?) -> Unit,
    onOpen: (account: String, rule: RuleWords?) -> Unit,
    onPreview: (account: String, new: Boolean, rule: RuleWords) -> Unit,
    onClose: () -> Unit,
) {
    var query by remember { mutableStateOf("") }
    val gesture = remember(words) { RuleGesture(words) }
    val trimmed = query.trim()
    val usable = accounts.usableOn(date)
    val recentOpen = recent.filter { it in usable }
    val matches = usable.filter { it.contains(trimmed, ignoreCase = true) }
    val run = { step: Step? ->
        when (step) {
            is Step.Save ->
                if (step.pick.new) onOpen(step.pick.account, step.rule)
                else onSelect(step.pick.account, step.rule)
            is Step.Preview -> onPreview(step.pick.account, step.pick.new, step.rule)
            null -> Unit
        }
    }
    ModalBottomSheet(onDismissRequest = onClose) {
        if (gesture.offered) RuleChoice(gesture, preview, run)
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            label = { Text(stringResource(R.string.account_search)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        )
        LazyColumn(modifier = Modifier.fillMaxWidth()) {
            if (trimmed.isEmpty() && recentOpen.isNotEmpty()) {
                item { Header(stringResource(R.string.account_recent)) }
                items(recentOpen, key = { "recent:$it" }) {
                    Account(it) { run(gesture.choose(Pick(it))) }
                }
                item { Header(stringResource(R.string.account_all)) }
            }
            if (isAccountName(trimmed) && accounts.none { it.name == trimmed }) {
                item {
                    ListItem(
                        colors = transparent(),
                        headlineContent = {
                            Text(stringResource(R.string.account_open_new, trimmed))
                        },
                        modifier =
                            Modifier.clickable { run(gesture.choose(Pick(trimmed, new = true))) },
                    )
                }
            }
            items(matches, key = { "all:$it" }) { Account(it) { run(gesture.choose(Pick(it))) } }
        }
    }
}

/**
 * The switch and, while it is on, the chips and the picked account with the rule's [preview] and
 * "Save"; [run] carries out what each step of [gesture] returns.
 */
@Composable
private fun RuleChoice(gesture: RuleGesture, preview: Int?, run: (Step?) -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier =
            Modifier.fillMaxWidth()
                .toggleable(value = gesture.on, role = Role.Switch, onValueChange = gesture::switch)
                .padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Text(
            stringResource(R.string.always_book),
            style = MaterialTheme.typography.bodyLarge,
            modifier = Modifier.weight(1f),
        )
        Spacer(Modifier.width(16.dp))
        Switch(checked = gesture.on, onCheckedChange = null)
    }
    if (gesture.on) {
        WordChips(gesture) { run(gesture.toggle(it)) }
        gesture.picked?.let { pick ->
            val rule = gesture.rule
            PickedAccount(pick, preview.takeIf { rule != null }, rule != null) {
                run(gesture.save())
            }
        }
    }
}

/** The words of counterparty and purpose under their headings, in text order. */
@Composable
private fun WordChips(gesture: RuleGesture, onToggle: (Pair<WordField, String>) -> Unit) {
    Column(modifier = Modifier.padding(horizontal = 16.dp)) {
        WordField.entries.forEach { field ->
            val words = gesture.wordsOf(field)
            if (words.isNotEmpty()) {
                Text(
                    stringResource(field.heading),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    words.forEach { word ->
                        val chip = field to word
                        FilterChip(
                            selected = chip in gesture.selected,
                            onClick = { onToggle(chip) },
                            label = { Text(word) },
                        )
                    }
                }
            }
        }
    }
}

/** The picked account with the rule's [preview] once it is known, and "Save" if [canSave]. */
@Composable
private fun PickedAccount(pick: Pick, preview: Int?, canSave: Boolean, onSave: () -> Unit) {
    ListItem(
        colors = transparent(),
        leadingContent = { Icon(painterResource(R.drawable.ic_check), contentDescription = null) },
        headlineContent = {
            Text(
                if (pick.new) stringResource(R.string.account_open_new, pick.account)
                else pick.account
            )
        },
        supportingContent =
            if (preview == null) null
            else {
                { Text(pluralStringResource(R.plurals.rule_preview, preview, preview)) }
            },
        trailingContent = {
            Button(onClick = onSave, enabled = canSave) { Text(stringResource(R.string.rule_save)) }
        },
    )
}

@Composable
internal fun Header(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelLarge,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
    )
}

@Composable
private fun Account(account: String, onClick: () -> Unit) {
    ListItem(
        colors = transparent(),
        headlineContent = { Text(account) },
        modifier = Modifier.clickable(onClick = onClick),
    )
}

/** List items on the sheet's own background. */
@Composable internal fun transparent() = ListItemDefaults.colors(containerColor = Color.Transparent)

/**
 * Whether [name] looks like a Beancount account, such as `Expenses:Garden`. Only a hint for the
 * "open" entry; the core decides and reports an invalid name as a conflict.
 */
internal fun isAccountName(name: String): Boolean = ACCOUNT.matches(name)

private val ACCOUNT =
    Regex("""(Assets|Liabilities|Equity|Income|Expenses)(:[\p{Lu}\p{N}][\p{L}\p{N}-]*)+""")

/** The field of the bank's original text that a word of a rule comes from, with its heading. */
internal enum class WordField(@param:StringRes val heading: Int) {
    Counterparty(R.string.rule_counterparty),
    Purpose(R.string.rule_purpose),
}

/** Whether the sheet offers "Always book like this": only for a text with a word to pick. */
internal fun showsSwitch(words: RuleWords): Boolean = !words.isEmpty

/**
 * The rule of the [selected] words, each field's in text order; `null` while none is selected,
 * because a rule needs at least one word.
 */
internal fun selectedRule(words: RuleWords, selected: Set<Pair<WordField, String>>): RuleWords? {
    val rule =
        RuleWords(
            counterparty = words.counterparty.filter { (WordField.Counterparty to it) in selected },
            purpose = words.purpose.filter { (WordField.Purpose to it) in selected },
        )
    return rule.takeUnless { it.isEmpty }
}

/** An account the user tapped; [new] if it is still to be opened. */
internal data class Pick(val account: String, val new: Boolean = false)

/** What the account sheet has to do after a step of the [RuleGesture]. */
internal sealed interface Step {
    /** Change the account to [pick]'s, with [rule] if it is not `null`. */
    data class Save(val pick: Pick, val rule: RuleWords?) : Step

    /** Count the other open transactions that [rule] would move along with [pick]. */
    data class Preview(val pick: Pick, val rule: RuleWords) : Step
}

/**
 * "Always book like this" in one opened account sheet, on the transaction's [words] (PRD: FR-D14).
 * The switch is off whenever the sheet opens, so a tap on an account saves at once, as without it.
 * Switched on, it selects the counterparty's words; a tap on an account then only picks it, and
 * "Save" saves it with the rule of the selected words. Each step returns what the sheet has to do.
 */
internal class RuleGesture(private val words: RuleWords) {
    /** Whether the sheet shows the switch at all. */
    val offered: Boolean = showsSwitch(words)

    var on by mutableStateOf(false)
        private set

    var selected by mutableStateOf(words.counterparty.map { WordField.Counterparty to it }.toSet())
        private set

    /** The account picked while the switch is on; `null` until a tap picks one. */
    var picked by mutableStateOf<Pick?>(null)
        private set

    /** The rule of the selected words; `null` while none is selected. */
    val rule: RuleWords?
        get() = selectedRule(words, selected)

    /** The words of [field] that the chips offer, in text order. */
    fun wordsOf(field: WordField): List<String> =
        when (field) {
            WordField.Counterparty -> words.counterparty
            WordField.Purpose -> words.purpose
        }

    /** Turns the switch [on] or off; off forgets the picked account. */
    fun switch(on: Boolean) {
        this.on = on
        if (!on) picked = null
    }

    /** A tap on [pick]: saves it while the switch is off, otherwise picks it for a preview. */
    fun choose(pick: Pick): Step? {
        if (!on) return Step.Save(pick, null)
        picked = pick
        return preview()
    }

    /** Selects [word] or, if it is selected, deselects it. */
    fun toggle(word: Pair<WordField, String>): Step? {
        selected = if (word in selected) selected - word else selected + word
        return preview()
    }

    /** "Save": the picked account with the rule; `null` while either is missing. */
    fun save(): Step? {
        val pick = picked ?: return null
        return rule?.let { Step.Save(pick, it) }
    }

    private fun preview(): Step? {
        val pick = picked ?: return null
        return rule?.let { Step.Preview(pick, it) }
    }
}
