package app.lentil.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.lentil.R

@Composable
fun ConnectScreen(viewModel: ConnectViewModel, onConnected: () -> Unit) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    LaunchedEffect(state.connected) { if (state.connected) onConnected() }
    Scaffold { padding ->
        Column(
            modifier =
                Modifier.fillMaxSize()
                    .padding(padding)
                    .verticalScroll(rememberScrollState())
                    .padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                stringResource(R.string.connect_title),
                style = MaterialTheme.typography.headlineSmall,
            )
            Text(
                stringResource(R.string.connect_intro),
                style = MaterialTheme.typography.bodyMedium,
            )
            Field(
                value = state.url,
                label = R.string.connect_url,
                keyboardType = KeyboardType.Uri,
                onChange = { value -> viewModel.update { it.copy(url = value) } },
            )
            Field(
                value = state.username,
                label = R.string.connect_username,
                onChange = { value -> viewModel.update { it.copy(username = value) } },
            )
            Field(
                value = state.token,
                label = R.string.connect_token,
                keyboardType = KeyboardType.Password,
                secret = true,
                onChange = { value -> viewModel.update { it.copy(token = value) } },
            )
            Field(
                value = state.authorName,
                label = R.string.connect_author_name,
                onChange = { value -> viewModel.update { it.copy(authorName = value) } },
            )
            Field(
                value = state.authorEmail,
                label = R.string.connect_author_email,
                keyboardType = KeyboardType.Email,
                onChange = { value -> viewModel.update { it.copy(authorEmail = value) } },
            )
            state.problem?.let { problem ->
                Text(
                    text =
                        when (problem) {
                            ConnectProblem.NotHttps -> stringResource(R.string.connect_not_https)
                            is ConnectProblem.Failed -> errorText(problem.error)
                        },
                    color = MaterialTheme.colorScheme.error,
                )
            }
            Button(
                onClick = viewModel::connect,
                enabled = state.complete && !state.connecting,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(
                    stringResource(
                        if (state.connecting) R.string.connecting else R.string.connect_button
                    )
                )
            }
        }
    }
}

@Composable
private fun Field(
    value: String,
    label: Int,
    onChange: (String) -> Unit,
    keyboardType: KeyboardType = KeyboardType.Text,
    secret: Boolean = false,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(stringResource(label)) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = keyboardType, autoCorrectEnabled = false),
        visualTransformation =
            if (secret) PasswordVisualTransformation()
            else androidx.compose.ui.text.input.VisualTransformation.None,
        modifier = Modifier.fillMaxWidth(),
    )
}
