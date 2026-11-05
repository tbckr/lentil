@file:OptIn(ExperimentalMaterial3Api::class)

package app.lentil.ui

import androidx.annotation.StringRes
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.lentil.R
import app.lentil.data.TagChoices
import app.lentil.data.TagSource

/**
 * Adds a tag to a transaction (PRD: FR-D12): the [choices] the core computed, each with the tags
 * picking it replaces, and the families, whose rest the user types. Without a readable `tags.toml`
 * the choices are the tags the ledger uses, under a hint.
 */
@Composable
fun TagSheet(
    choices: TagChoices,
    isPlainName: (String) -> Boolean,
    onPick: (String) -> Unit,
    onClose: () -> Unit,
) {
    var family by remember { mutableStateOf<String?>(null) }
    var rest by remember { mutableStateOf("") }
    ModalBottomSheet(onDismissRequest = onClose) {
        Header(stringResource(R.string.tag_picker_title))
        sourceHint(choices.source)?.let { hint ->
            Text(
                stringResource(hint),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
            )
        }
        LazyColumn(modifier = Modifier.fillMaxWidth()) {
            if (choices.tags.isEmpty() && choices.families.isEmpty()) {
                item {
                    ListItem(
                        colors = transparent(),
                        headlineContent = { Text(stringResource(R.string.tag_picker_empty)) },
                    )
                }
            }
            items(choices.tags, key = { "tag:${it.tag}" }) { choice ->
                val replaces = choice.replaces.joinToString(", ") { "#$it" }
                ListItem(
                    colors = transparent(),
                    headlineContent = { Text("#${choice.tag}") },
                    supportingContent =
                        if (replaces.isEmpty()) null
                        else {
                            { Text(stringResource(R.string.tag_replaces, replaces)) }
                        },
                    modifier = Modifier.clickable { onPick(choice.tag) },
                )
            }
            if (choices.families.isNotEmpty()) {
                item { Header(stringResource(R.string.tag_families)) }
                items(choices.families, key = { "family:$it" }) { prefix ->
                    ListItem(
                        colors = transparent(),
                        headlineContent = { Text("#$prefix…") },
                        modifier =
                            Modifier.clickable {
                                family = prefix
                                rest = ""
                            },
                    )
                }
            }
            family?.let { prefix ->
                val tag = familyTag(prefix, rest.trim(), isPlainName)
                item {
                    OutlinedTextField(
                        value = rest,
                        onValueChange = { rest = it },
                        prefix = { Text(prefix) },
                        label = { Text(stringResource(R.string.tag_family_rest)) },
                        isError = breaksNameRule(rest.trim(), isPlainName),
                        singleLine = true,
                        keyboardOptions = NAME_KEYBOARD,
                        keyboardActions = KeyboardActions(onDone = { tag?.let(onPick) }),
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                    )
                }
                item {
                    TextButton(
                        onClick = { tag?.let(onPick) },
                        enabled = tag != null,
                        modifier = Modifier.padding(horizontal = 8.dp),
                    ) {
                        Text(stringResource(R.string.tag_family_add, prefix + rest.trim()))
                    }
                }
            }
        }
    }
}

/** The tag [family] + [rest], if [rest] may follow a family prefix by the core's rule. */
internal fun familyTag(family: String, rest: String, isPlainName: (String) -> Boolean): String? =
    if (isPlainName(rest)) family + rest else null

/** Whether the field should show an error: [text] is not empty and breaks the core's rule. */
internal fun breaksNameRule(text: String, isPlainName: (String) -> Boolean): Boolean =
    text.isNotEmpty() && !isPlainName(text)

/**
 * The keyboard for trip names and family rests: no capitals and no autocorrect, which would turn
 * `lisbon` into `Lisbon` and break the core's rule, and Done submits.
 */
internal val NAME_KEYBOARD =
    KeyboardOptions(
        capitalization = KeyboardCapitalization.None,
        autoCorrectEnabled = false,
        keyboardType = KeyboardType.Ascii,
        imeAction = ImeAction.Done,
    )

/** The hint over choices that don't come from `tags.toml`, `null` for those that do. */
@StringRes
internal fun sourceHint(source: TagSource): Int? =
    when (source) {
        TagSource.Taxonomy -> null
        TagSource.Missing -> R.string.tag_source_missing
        TagSource.Invalid -> R.string.tag_source_invalid
    }
