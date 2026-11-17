# The AI chat in Corelens Desktop AI

*Ask AI*, bottom right of the window (or **Ctrl+Shift+A**), opens a chat
about the project open in Corelens. It reads the project and the devices
with the same tools Claude Code and Claude Desktop use through MCP
(`docs/MCP.md`), under the same checks: only this project's devices,
read-only commands only, every result redacted, and a change only ever
proposed, for you to approve in the Corelens window. It runs on the provider
you choose: Anthropic, OpenAI (or an OpenAI-compatible server), Ollama, or
Snowflake Cortex.

It is **off until you turn it on**. Until then Corelens makes no request to
any provider at all.

## 1. Choose a provider

*Settings → AI*:

1. **AI chat in Corelens** — tick *Turn on AI chat in Corelens*, choose the
   provider.
2. **AI providers** — set that provider up:
   - **Anthropic.** Copy an API key from the Claude Console, then press
     *Paste key from the clipboard*. Corelens reads the clipboard, keeps the
     key in the system keychain (Windows Credential Manager) and empties the
     clipboard; the key is never shown, logged or exported. *Refresh models*
     lists the models the key can use; the default is `claude-opus-5-5`, and
     any model id can be typed.
   - **OpenAI.** The same, with a key from the OpenAI dashboard. The default
     API is *Responses*, which OpenAI's current models need for tools. For
     an OpenAI-compatible server (Azure OpenAI, or a local server), set the
     *Base URL* and, where the server has no Responses API, choose *Chat
     Completions*. An Azure host gets its `api-key` header automatically.
   - **Ollama.** No key. The base URL defaults to `http://localhost:11434`.
     *Refresh models* lists the models pulled there and says which cannot
     call tools — the chat needs one that can (for example `qwen3`,
     `llama3.1`, `mistral-small3.1` or `gpt-oss`).
   - **Snowflake Cortex.** Type the account identifier
     (`orgname-accountname`, from Snowsight → your name → Account → View
     account details). Create a programmatic access token in Snowsight
     (your name → Settings → Authentication), copy it and press *Paste key
     from the clipboard*; it is kept in the keychain like a key. The token's
     user needs a default role granted `SNOWFLAKE.CORTEX_USER`. Corelens
     uses Cortex's OpenAI-compatible Chat Completions
     (`https://<account>.snowflakecomputing.com/api/v2/cortex/v1`); tools
     work on Cortex's Claude and OpenAI models, so choose one of those
     (default `claude-sonnet-5-5`). A model your region does not serve may
     still be reached through Snowflake's cross-region inference, which the
     account administrator turns on.

   The model is a list: the models a provider is known to offer, and after
   *Refresh models* the ones it lists, with *Another model ID…* for one
   typed by hand. Choosing a model under the chat or under its provider is
   the same choice.
3. **Test connection** (under the chat) asks the provider for its models
   and says whether the chosen one is among them.

## 2. Ask

Type a question and press Enter (Shift+Enter for a new line). The answer
streams in; each tool the model calls is shown with its device and command,
and the rows a template parsed from a command's output appear as a table.
*Stop* ends the answer — and withdraws a change it was waiting on.

From the diagram, right-click a device or a link and choose *Ask AI about
this device* / *about this link*; the inspector's *Ask AI* and a finding's
*Ask AI about this* do the same. The question then starts with what it is
about.

The panel floats and can be resized, or docked down the right (the pin).
The conversation belongs to the project and is still there when the project
is opened again; *Settings → AI → Retention and logging* keeps it 90 days
unless you choose 30 or 365, and clears it.

## 3. Changes

When the model wants to change a device it calls `propose_config_change`,
and the approval dialog opens over the window — exactly as for a change
Claude Code proposes. The chat shows the proposal waiting, with *Open
approval*. Approve, deny, or let it time out (10 minutes, then denied). The
dialog follows the change through its push, verification and any rollback,
and *Settings → AI → Configuration changes* keeps the record, exported as
Markdown or CSV. Saving the configuration is always a separate proposal.

## 4. What it costs and what is sent

- Each answer can call at most 15 tools (*Settings*); past that it is told
  to answer with what it has.
- A tool's output longer than 12 000 characters is cut, the beginning and
  the end kept and the cut said, before it is sent.
- The header shows the tokens of the last answer and of the whole
  conversation, as the provider counted them.
- Everything sent passes the redaction first: passwords, secrets, SNMP
  communities, keys and hashes removed, every vault secret removed wherever
  it appears. *AI providers → Show last request* shows the last call made
  to that provider, whether a question, **Refresh models** or **Test
  connection**. It lists the method and address, the headers with the key
  shown as `<redacted>`, the body as sent, the status and what came back.

## 5. When it does not answer

The error says what is wrong, in words:

| It says | What to do |
| --- | --- |
| The API key was refused | Paste a working key. |
| No credit or quota left | Top up the provider account. |
| That model is not available | Choose one *Refresh models* lists. |
| Ollama is not answering at … | Start Ollama (the app, or `ollama serve`). |
| … cannot call tools | Pull and choose a model that can. |
| Rate limited … try again in N seconds | Wait that long (Corelens retries twice by itself first). |
| AI chat in Corelens is off | Turn it on in *Settings → AI*. |

## 6. Diagnose

Ask "check this network, what's going on", "run a full diag", or about a
site, a device or a path, and the model runs `run_diagnostic` — Corelens's
own read-only sweep — before it says anything. It then explains from the
report, citing its evidence, may run a few more show commands to confirm
what the report leaves open, and says when there is not enough data. The
fix plan arrives as proposals in the approval dialog, one per step, as it
does from the Diagnose button (`docs/DIAGNOSE.md`); the chat points to them
rather than proposing them again. *Ask AI to explain* on a report opens the
chat with a question about it, for you to send.
