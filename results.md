# Moon Rabbit — implementation and verification results

Updated 2026-10-15. The current feature changes supersede the earlier mixed-direction and text-file-import behavior. Backend word/persistence/speech, AI transport, and frontend resources/UI were implemented in parallel, then integrated and exercised in Chromium against real isolated LocalStack storage.

## Requested changes

| Request                                  | Delivered behavior                                                                                                                                                                                                                                                                                                                                                   |
| ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Angular HTTP resources                   | All HTTP reads use real Angular `httpResource`, including the read-only POST session query. Dashboard, queue/list summaries, paged lists, word details, search, and session launch expose resource loading/error/value/reload state. Writes, AI commands, speech, and durable grade delivery remain explicit HttpClient requests, as approved during implementation. |
| Remove, not retain, AWS resources        | DynamoDB and S3 use `RemovalPolicy.DESTROY`; S3 has `autoDeleteObjects: true`. A stack aspect applies the same policy to CDK-generated support resources. Synthesized deletion and replacement policies are `Delete` for every resource.                                                                                                                             |
| Direction selector                       | A two-option selector displays exactly `chinese-english` and `english-chinese`. Each session emits only the selected direction, not both. Requests use `direction`, not the removed `production` boolean.                                                                                                                                                            |
| Search memberships and voice             | Results show the visible system/personal lists containing the word and a Play control. Existing dictionary entries without visible memberships are distinguished from missing entries in screenshot previews.                                                                                                                                                        |
| New-word pronounced pinyin               | Manual creation and AI fill expose separate formal and pronounced pinyin fields. Pronounced pinyin is editable and stored for new words; speech uses it.                                                                                                                                                                                                             |
| Screenshot import instead of text upload | Text-file import is removed. PNG/JPEG/WebP screenshots produce a fully editable AI preview of vocabulary/readings and source sentences, with dictionary matches and list memberships. A personal target list is selected before saving. Confirmed rows disappear; rejected/unconfirmed rows remain for correction/retry.                                             |
| Example sentences                        | Words store examples with Chinese text, formal pinyin, pronounced pinyin, English translation, and stable IDs. Screenshot source sentences are merged into existing/new words. Word details and revealed study cards show examples, a generate-and-save HSK1 button, and sentence voice controls.                                                                    |

### Angular resource boundary

The user selected **resources for reads; HttpClient for writes** after reviewing Angular's [official mutation guidance](https://angular.dev/guide/http/http-resource). Reactive resources cancel obsolete requests; using that lifecycle for durable writes can be unsafe. Session selection does not persist anything, so its POST is also a resource. Grading retains its durable FIFO/Web Locks delivery rather than being made reactive.

Resource values are guarded with `hasValue()` before reading. Search and detail requests are disabled when their intent clears/closes; routing/pagination drive request signals. Session initialization consumes the resolved response only once for the captured launch, using untracked mutable session initialization so subsequent answers cannot reset the session.

### Destructive AWS lifecycle

The synthesized stack contains six resources: DynamoDB table, S3 bucket, bucket policy, object-deletion custom resource, provider IAM role, and provider Lambda. **All six have both `DeletionPolicy: Delete` and `UpdateReplacePolicy: Delete`.**

**Stack destruction/replacement can now delete learning data and empty/delete the audio bucket.** No deployment or destruction of the user's stack was performed. Existing deployed stacks need the updated template deployed for these policies to take effect. The local initialization script creates standalone LocalStack resources; CloudFormation removal policies apply to CDK-managed resources, not independently created resources.

## Word and screenshot behavior

- `AI_MODEL` is captured in configuration, defaulting to `google/gemini-2.5-flash`; `.env.example` includes it. All three AI operations share one structured OpenRouter transport: word fill, screenshot extraction, and example generation.
- Both pinyins are requested even when their values are equal. Word fill does not invent source examples.
- Screenshots are sent to the configured OpenRouter model/provider for extraction. The screenshot itself is not persisted to DynamoDB/S3. Only explicitly saved vocabulary/examples are stored.
- Image requests must be PNG, JPEG, or WebP base64 data URLs, with matching decoded magic and at most 8 MiB decoded bytes. Remote image URLs and SVG are rejected.
- The screenshot route alone accepts a larger, 12 MiB JSON body; ordinary routes retain the 1 MiB limit.
- Extraction asks for all listed vocabulary, or segmented vocabulary when the source consists of sentences. Explicit source sentences are associated with their matching readings, with both pinyins and translation filled. When no source sentence is present, the preview has no invented example.
- Output is bounded to 200 vocabulary items, 12 readings per item, and 20 source sentences per reading. Unreadable/dense/incomplete/truncated/refused/malformed model results are rejected explicitly instead of being silently imported as a partial result. A valid empty extraction produces an empty preview.
- Every reading becomes its own editable preview row. All four sentence fields can be edited before saving. Preview pages contain at most 40 rows; extraction-time matches are labeled as snapshots, because editing an identity can change the final match.
- Saving uses at most 40 words **and a JSON byte budget below the ordinary HTTP body limit**. This matters when source sentences are long. Each batch is committed against the captured destination; changing routes cannot silently retarget in-flight work or errors.
- Re-adding an existing word preserves its canonical dictionary fields and existing examples. New source examples are merged even if destination membership already exists. Stable IDs make repeated source imports idempotent; limits fail without removing existing examples.
- Stored examples survive API process restart. Legacy word records default to an empty example collection without a bulk rewrite or word-ID change.
- Sentence payloads are fetched on the word-detail resource, not duplicated in every list/search response. This keeps the new storage feature from making paged lists unnecessarily large.

### Example generation and speech

Generated examples must contain the exact target word and only Hanzi from the authoritative HSK1 list, plus ordinary punctuation/whitespace. This is validated server-side, not merely requested in a prompt. If the word itself contains characters outside HSK1, generation returns an actionable 422 instead of silently relaxing the requirement or generating an unrelated sentence. Screenshot source sentences are not subject to this generated-example constraint.

Generation persists the validated result atomically using the catalog revision protocol. Existing examples are preserved; duplicate generated sentences and exhausted storage limits return explicit errors.

Speech accepts only a word ID and optional stored sentence ID. It resolves authoritative text/pronounced pinyin, not arbitrary client-provided speech text. Sentence pronunciation handles punctuation and uses distinct cache identities. Fish phoneme control, shared cached work, stale playback protection, object-URL cleanup, and browser-cache recovery remain in place.

Protocol references: [OpenRouter image inputs](https://openrouter.ai/docs/guides/overview/multimodal/image-understanding.md), [configured model](https://openrouter.ai/google/gemini-2.5-flash), [Fish Chinese phoneme control](https://docs.fish.audio/developer-guide/core-features/fine-grained-control/chinese).

## Integration defects found and fixed

1. **Sentence-heavy imports exceeded the HTTP body limit despite respecting the 40-word count.** Reproduced against the actual API: eight preview rows produced HTTP 413 and all remained pending. Added byte-bounded batching; the same browser scenario then completed as two HTTP 200 requests with no rows left pending. A permanent regression covers this boundary.
2. **Production-only history was excluded from studied-entry/words-left counts.** Reproduced with two real English-to-Chinese answers: only production cards were stored, but introduction counts did not change. Aggregation now counts a distinct word answered in either direction. A regression prevents double counting when both directions exist; the live corrected dashboard showed the expected studied character and remaining-word counts.
3. **Sentence data inflated list responses even though list rows do not render it.** Moved examples to the word-detail contract/resource. Canonical persistence and card behavior are unchanged.
4. **Screenshot matching repeatedly normalized/scanned the entire dictionary.** Reused normalized word identity and direct catalog lookup instead. Membership labels retain visibility filtering and stable ordering.
5. **Infrastructure defaults could retain generated support resources.** Applied one stack-wide destructive policy as well as explicit table/bucket configuration; verified the emitted template, not only source literals.

### Measured list-response size

Same ten-word test list, including deliberately sentence-heavy fixtures; decoded response body measured in Chromium:

| Projection                           |     Bytes |
| ------------------------------------ | --------: |
| Examples duplicated in list rows     | 1,215,558 |
| Examples fetched only by word detail |     1,601 |

This is a payload-size observation, not a controlled latency benchmark.

## Verification

### Executable checks

| Check                                               | Observed result                                                                                                                                                                                                                                                           |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm test`                                         | 50 Rust unit tests passed; 2 integration tests intentionally ignored in the unit invocation. All 34 client regressions passed.                                                                                                                                            |
| `pnpm test:persistence`                             | Both real LocalStack regressions passed. The script now runs all ignored persistence tests, including repeated-membership example merging, reload persistence, duplicate generation rejection, and catalog CAS conflict, in addition to concurrent grade/queue integrity. |
| `cargo build --manifest-path apps/api/Cargo.toml`   | Passed on the final implementation.                                                                                                                                                                                                                                       |
| `pnpm exec nx build web --skip-nx-cache`            | Final production build passed.                                                                                                                                                                                                                                            |
| ESLint over web/contracts/infra, `--max-warnings=0` | Passed with zero errors and warnings.                                                                                                                                                                                                                                     |
| `pnpm exec tsc -p apps/infra/tsconfig.json`         | Passed.                                                                                                                                                                                                                                                                   |
| CDK synthesis                                       | All six emitted resources have Delete deletion/replacement policies; S3 object deletion support is emitted.                                                                                                                                                               |

### Chromium and actual API/storage scenarios

Testing used the production-built frontend and the real API with an **isolated DynamoDB table and audio bucket**, so source-example fixtures could not modify the original dictionary/history. The original `http://localhost:9851/` was also checked read-only.

- Home resource failure/retry surface while the isolated API seeded, followed by successful navigation and data rendering.
- Exact two-option direction selector on both isolated and original apps. A real English-to-Chinese session completed one remembered/one missed answer; DynamoDB contained two production cards, no recognition cards from that session, and zero pending confirmations.
- Examples were absent from the study question side and available after revealing the answer. Missing-key generation was actionable; words outside HSK1 displayed the server's explicit constraint error.
- Actual search for 你好 displayed seven HSK memberships and a working Play action/error surface. Native search clearing removed all results. Consumer regressions cover obsolete-resource intent and error-state guards.
- Real screenshot upload with no provider key displayed the configuration error and saved nothing. A valid approximately 1.65 MB image request reached the screenshot handler, whereas the same-sized ordinary request returned 413. Invalid image input returned 400.
- Controlled AI-result fixtures drove the positive screenshot UI: existing/new entry indicators, both pinyins, editable source sentences, and personal target selection. Saving used the real API/DynamoDB. The selected target received the two words and edited source example; the originating list stayed unchanged.
- The dense-sentence failure/fix scenario above used real API writes, not acknowledged mock persistence.
- AI-filled manual pronunciation was displayed, edited, saved, and read back from DynamoDB. Formal `yuè liàng tù` and spoken `yuè liang tù` remained separate.
- Source sentences and their fields survived API restart and word-detail reload. The generated-result UI success path was exercised with a controlled model-result fixture backed by a real word/example save; reopening the card showed both persisted examples.
- Real sentence Play traversed the stored-sentence API/S3 cache and native browser decoder using an explicitly synthetic silent MP3 fixture: HTTP 200, `playing`, readyState 4, finite duration, then `ended`, without an alert. This verifies playback plumbing, **not Mandarin pronunciation**. Unknown stored sentence IDs returned 404.
- Modal Tab and Shift+Tab remained inside the expanded dialog, Escape closed it, and opener focus was restored.
- At 375 px, the screenshot preview displayed both rows and source sentences with no horizontal overflow. Text-file controls were absent. A system-list page could choose a personal target.
- Actual browser back/forward between list routes showed no preview in the other editor and restored the original two-row preview with its selected target on return.
- Original app still rendered at 375 px without overflow and showed the new selector. Its existing answer accuracy and studied-character count remained readable. No assistant grades or dictionary mutations were written to the original table.
- No unexpected JavaScript page error was observed in the final desktop/mobile/original-app checks. Intentional 400/404/413/422/503 responses were part of the boundary/failure scenarios.

### Provider verification limit

**This app has no available `OPENROUTER_API_KEY` in the verification environment.** No unrelated project's credential was borrowed. Real paid OCR/enrichment/generation, model linguistic accuracy, and audible Mandarin/polyphonic sentence pronunciation therefore remain unverified.

Positive AI UI scenarios used explicitly controlled result fixtures. Successful storage/reload/batching and cached-media playback used real LocalStack and the real application. Provider request construction, structured-output completeness, image validation, HSK1 enforcement, punctuation, and cache distinctions are covered by regressions. No fake provider fallback or stub was added to production code.

To use live AI, configure this application's own key with model access/credits and restart the API. `AI_MODEL` must support image input and structured JSON output. Preview drafts survive reused list routes during the active visit, not full browser refresh/closure.

## Earlier repair/review status retained

The previous repair work remains in place; current exclusive directions and screenshot import replace the earlier mixed-direction/text-file policies.

| Original findings                  | Current disposition                                                                                                                                                      |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| F01 mature review scheduling       | Explicit FSRS transitions, elapsed memory forecasting, same-day updates, one lapse per relearning episode, single interval fuzz, and explicit inference errors retained. |
| F02/F06 client grade loss/order    | Durable cross-tab FIFO, event-ID acknowledgements, lost-wakeup fix, and retained stale/corrupt recovery entries retained.                                                |
| F03 atomic server grades           | Conditional event/card/day transaction and concurrency regressions retained.                                                                                             |
| F04 direction backfill             | Missing selected directions backfill studied words independently of queue/due status; now works exclusively in either selected direction.                                |
| F05/F07 clock and grade validation | Future-event bounds, old future-card recovery, required IDs/word existence, and chronology retained.                                                                     |
| F08/F09 queue/session races        | Queue versioning/single-flight and immutable guarded session launch retained; launch is now resource-driven.                                                             |
| F10 obsolete responses             | Resource-driven reads and revision/list-owned mutation state replace manual read subscriptions.                                                                          |
| F11 imports                        | Text upload removed; screenshot previews retain all extracted rows/readings and acknowledged/rejected batch semantics.                                                   |
| F12 timers                         | Exit, expiry, destruction, and unanswered-card semantics retained.                                                                                                       |
| F13/F14 catalog/provider I/O       | Publish-after-commit and no provider network I/O under catalog guards retained.                                                                                          |
| F15/F16/F17 speech/audio           | Retained completion, pronunciation-aware requests/cache, cancellation, and URL/cache cleanup extended to sentences.                                                      |
| F18/F19 accessibility/mobile       | Named controls, keyboard-modal details, focus restoration, and scoped mobile layout retained and checked with the new UI.                                                |
| F20 large lists                    | 100-word server/client pagination retained; examples now load only for detailed cards.                                                                                   |
| S01/S02 local/deployment scope     | Runtime remains explicitly local-only; Host/Origin checks are not authentication. No public/multi-user serving claim or Lambda fallback.                                 |
| S03 credentials                    | Repository-scoped dotenv and captured provider configuration retained; no borrowed credentials.                                                                          |
| S04 multiple writers               | Catalog revision/snapshot/CAS, queue conflict detection, and concurrent bootstrap protections retained.                                                                  |
| S05 dependency advisory            | Prior Angular/Rust dependency hardening retained. No dependency security scan was rerun for this feature delivery.                                                       |

The scheduler remains `fsrs` 5.2.0 with explicit FSRS-6 defaults and a 0.9 target. Recognition and production keep independent card memory, although introduced-word counters now deduplicate across them. Dashboard answer accuracy is still not model retention. Previously lost review events cannot safely be reconstructed.

The prior Rust audit documented two upstream informational maintenance warnings (`bincode` 2.0.1 and `paste` 1.0.15 through FSRS/Burn), with no advertised patched versions. Those upstream maintenance risks remain; changing the scheduling implementation solely to hide them was not part of this delivery.

## Cleanup and deployment boundary

Assistant-owned browser tabs, temporary frontend/API services, screenshot/audio fixtures, synthesized verification assets, isolated DynamoDB table, and isolated S3 bucket/objects were removed after verification. The two persistence-test tables were deleted by their passing tests. The user's original services on ports 9850/9851 were left running.

This is still a local, process-configured single-profile application. Do not expose it through a public proxy/tunnel as if Host/Origin validation were user authentication. A production/multi-user deployment needs a separate identity, authorization, storage/configuration, and deployment design.
