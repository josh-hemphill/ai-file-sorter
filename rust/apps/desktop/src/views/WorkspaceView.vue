<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import TabBar from "../components/TabBar.vue";
import WorkflowStepper from "../components/WorkflowStepper.vue";
import AnalysisStream from "../components/AnalysisStream.vue";
import PreviewDiff from "../components/PreviewDiff.vue";
import ApplyConfirm from "../components/ApplyConfirm.vue";
import {
  applyPlan,
  chatRevision,
  connectEngine,
  onEngineLog,
  onEngineProgress,
  patchRevision,
  pickFolder,
  planRevision,
  proposeSession,
  scanRoot,
  undoJournal,
} from "../engine";
import { destinationTree } from "../tree";
import type {
  ApplyJournal,
  CenterView,
  ChatLine,
  IntentPreset,
  LogEvent,
  ObservedEntry,
  OperationPlan,
  PlanIssue,
  ProgressEvent,
  ProposalRevision,
  WorkspaceSnapshot,
} from "../types";
import {
  INTENT_PRESETS,
  acceptedCount,
  applyConfirmCopy,
  constraintLabel,
  currentWorkflowStep,
  issueLabel,
  itemRows,
  loadRecentRoots,
  persistRecentRoots,
  planCounts,
  previewRows,
  rememberRoot,
  roleKindLabel,
  skippedReasonLabel,
} from "../workflow";

const STREAM_CAP = 1000;
const STAGE_ORDER = [
  { id: "scan", label: "Walk" },
  { id: "relationships", label: "Relationships" },
  { id: "extract", label: "Metadata" },
];

const emit = defineEmits<{
  "open-settings": [];
}>();

const engineReady = ref(false);
const engineError = ref<string | null>(null);
const busy = ref(false);
const progress = ref<ProgressEvent | null>(null);
const rootPath = ref("");
const recentRoots = ref<string[]>(loadRecentRoots());
const preset = ref<IntentPreset>("inbox");
const view = ref<CenterView>("structure");
const snapshot = ref<WorkspaceSnapshot | null>(null);
const revision = ref<ProposalRevision | null>(null);
const plan = ref<OperationPlan | null>(null);
const issues = ref<PlanIssue[]>([]);
const journal = ref<ApplyJournal | null>(null);
const confirmApply = ref(false);
const selectedAsset = ref<string | null>(null);
const query = ref("");
const draft = ref("");
const chatLog = ref<ChatLine[]>([]);
const logLines = ref<LogEvent[]>([]);
const stageProgress = ref<Record<string, { current: number; total: number | null; message: string }>>(
  {},
);

const tree = computed(() => destinationTree(revision.value));
const files = computed(
  () => snapshot.value?.entries.filter((entry) => entry.kind === "file") ?? [],
);
const filteredFiles = computed(() => {
  const needle = query.value.trim().toLowerCase();
  if (!needle) {
    return files.value;
  }
  return files.value.filter((entry) => {
    const dest = placementFor(entry.id)?.destination ?? "";
    return (
      entry.path.toLowerCase().includes(needle) || dest.toLowerCase().includes(needle)
    );
  });
});
const rows = computed(() => itemRows(filteredFiles.value, snapshot.value));
const selectedEntry = computed(
  () => files.value.find((entry) => entry.id === selectedAsset.value) ?? null,
);
const selectedEvidence = computed(() => {
  const id = selectedAsset.value;
  if (!id || !snapshot.value) {
    return [];
  }
  return snapshot.value.evidence.filter((bag) => bag.asset === id);
});
const errorIssues = computed(() =>
  issues.value.filter((issue) => issue.severity === "error"),
);
const step = computed(() =>
  currentWorkflowStep({
    snapshot: snapshot.value,
    revision: revision.value,
    plan: plan.value,
    issues: issues.value,
    journal: journal.value,
  }),
);
const approved = computed(() => acceptedCount(revision.value));
const counts = computed(() => planCounts(plan.value));
const diffs = computed(() => previewRows(plan.value, journal.value));
const confirmSummary = computed(() =>
  applyConfirmCopy(snapshot.value?.root ?? rootPath.value, counts.value),
);
const tabCounts = computed(() => ({
  items: files.value.length,
  relationships: snapshot.value?.bundles.length ?? 0,
  activity: logLines.value.length + issues.value.length + diffs.value.length,
}));
const analysisStages = computed(() =>
  STAGE_ORDER.filter((stage) => stageProgress.value[stage.id]).map((stage) => ({
    ...stage,
    ...stageProgress.value[stage.id],
  })),
);

watch(recentRoots, (paths) => persistRecentRoots(paths), { deep: true });

function placementFor(asset: string) {
  return revision.value?.placements[asset];
}

function choosePreset(id: IntentPreset) {
  preset.value = id;
  if (id === "custom") {
    emit("open-settings");
  }
}

async function chooseFolder() {
  engineError.value = null;
  try {
    const picked = await pickFolder();
    if (picked) {
      rootPath.value = picked;
    }
  } catch (error) {
    engineError.value = String(error);
  }
}

async function runScan() {
  if (!rootPath.value) {
    engineError.value = "Choose a source folder first.";
    return;
  }
  busy.value = true;
  engineError.value = null;
  plan.value = null;
  journal.value = null;
  issues.value = [];
  logLines.value = [];
  stageProgress.value = {};
  view.value = "activity";
  try {
    await connectEngine();
    engineReady.value = true;
    const next = await scanRoot(rootPath.value, preset.value);
    snapshot.value = next;
    recentRoots.value = rememberRoot(recentRoots.value, rootPath.value);
    const proposed = await proposeSession(next.session, preset.value);
    revision.value = proposed;
    view.value = "structure";
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
  }
}

async function setReview(asset: string, review: "accepted" | "rejected" | "proposed") {
  return setReviewAssets([asset], review);
}

async function setReviewAssets(
  assets: string[],
  review: "accepted" | "rejected" | "proposed",
) {
  if (!snapshot.value || !revision.value || assets.length === 0) {
    return;
  }
  const op =
    review === "accepted" ? "accept" : review === "rejected" ? "reject" : "reopen";
  busy.value = true;
  try {
    revision.value = await patchRevision(
      snapshot.value.session,
      revision.value.id,
      `${op} ${assets.length} items`,
      [{ op, assets }],
    );
    plan.value = null;
    issues.value = [];
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
  }
  if (
    revision.value &&
    acceptedCount(revision.value) > 0 &&
    !Object.values(revision.value.placements).some(
      (placement) => placement.review === "proposed",
    )
  ) {
    await validatePlan();
  }
}

async function acceptAll() {
  if (!snapshot.value || !revision.value) {
    return;
  }
  const assets = Object.keys(revision.value.placements);
  busy.value = true;
  try {
    revision.value = await patchRevision(
      snapshot.value.session,
      revision.value.id,
      "accept all",
      [{ op: "accept", assets }],
    );
    plan.value = null;
    issues.value = [];
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
  }
  if (revision.value && acceptedCount(revision.value) > 0) {
    await validatePlan();
  }
}

async function validatePlan() {
  if (!snapshot.value || !revision.value) {
    return;
  }
  busy.value = true;
  try {
    const result = await planRevision(snapshot.value.session, revision.value.id);
    plan.value = result.plan;
    issues.value = result.issues;
    view.value = "activity";
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
  }
}

async function requestApply() {
  if (!snapshot.value || !plan.value) {
    engineError.value = "Validate a plan before preview or apply.";
    return;
  }
  if (errorIssues.value.length > 0) {
    engineError.value = "Fix plan errors before applying.";
    return;
  }
  confirmApply.value = true;
}

async function confirmAndApply() {
  confirmApply.value = false;
  await runApply(false);
}

async function runApply(dryRun: boolean) {
  if (!snapshot.value || !plan.value) {
    engineError.value = "Validate a plan before preview or apply.";
    return;
  }
  if (errorIssues.value.length > 0) {
    engineError.value = "Fix plan errors before applying.";
    return;
  }
  busy.value = true;
  try {
    journal.value = await applyPlan(snapshot.value.session, plan.value.id, dryRun);
    view.value = "activity";
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
  }
}

async function runUndo() {
  if (!snapshot.value || !journal.value) {
    return;
  }
  busy.value = true;
  try {
    journal.value = await undoJournal(snapshot.value.session, journal.value.id);
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
  }
}

async function sendChat() {
  const text = draft.value.trim();
  if (!text) {
    return;
  }
  if (!snapshot.value || !revision.value) {
    engineError.value = "Scan a source before chatting.";
    return;
  }
  busy.value = true;
  engineError.value = null;
  chatLog.value.push({ role: "you", text });
  draft.value = "";
  try {
    const reply = await chatRevision(
      snapshot.value.session,
      revision.value.id,
      text,
    );
    chatLog.value.push({ role: "assistant", text: reply.message });
    if (reply.revision) {
      revision.value = reply.revision;
      plan.value = null;
      issues.value = [];
    }
  } catch (error) {
    engineError.value = String(error);
    chatLog.value.push({
      role: "assistant",
      text: `That turn failed: ${String(error)}`,
    });
  } finally {
    busy.value = false;
  }
}

function renderTree(node: ReturnType<typeof destinationTree>, depth = 0): string {
  if (depth === 0) {
    return node.children.map((child) => renderTree(child, 1)).join("");
  }
  const pad = "  ".repeat(depth - 1);
  const files = node.fileCount ? ` · ${node.fileCount}` : "";
  const kids = node.children.map((child) => renderTree(child, depth + 1)).join("");
  return `${pad}${node.name}${files}\n${kids}`;
}

let stopProgress: (() => void) | undefined;
let stopLog: (() => void) | undefined;
onMounted(async () => {
  stopProgress = await onEngineProgress((event) => {
    progress.value = event;
    stageProgress.value = {
      ...stageProgress.value,
      [event.stage]: {
        current: event.current,
        total: event.total,
        message: event.message,
      },
    };
  });
  stopLog = await onEngineLog((event) => {
    const next = logLines.value.concat(event);
    logLines.value = next.length > STREAM_CAP ? next.slice(next.length - STREAM_CAP) : next;
  });
  try {
    await connectEngine();
    engineReady.value = true;
  } catch (error) {
    engineError.value = String(error);
  }
});
onUnmounted(() => {
  stopProgress?.();
  stopLog?.();
});

function familyOf(entry: ObservedEntry): string {
  return entry.family.replace(/_/g, " ");
}
</script>

<template>
  <div class="shell">
    <aside class="rail left">
      <header class="brand">
        <strong>Workspace</strong>
        <span class="muted">{{ engineReady ? "engine ready" : "engine offline" }}</span>
      </header>
      <p class="muted safety">Nothing is moved until you Apply.</p>
      <label class="field">
        Source
        <div class="row">
          <input v-model="rootPath" placeholder="/path/to/folder" :title="rootPath" />
          <button type="button" @click="chooseFolder">Browse</button>
        </div>
      </label>
      <p class="label">Intent</p>
      <button
        v-for="item in INTENT_PRESETS"
        :key="item.id"
        type="button"
        class="preset"
        :class="{ active: preset === item.id }"
        @click="choosePreset(item.id)"
      >
        <strong>{{ item.label }}</strong>
        <span>{{ item.hint }}</span>
      </button>
      <p class="label">Recent</p>
      <button
        v-for="item in recentRoots"
        :key="item"
        type="button"
        class="recent"
        :title="item"
        @click="rootPath = item"
      >
        {{ item }}
      </button>
      <p v-if="!recentRoots.length" class="muted">Scanned folders show up here.</p>
      <button class="primary" type="button" :disabled="busy" @click="runScan">
        Scan
      </button>
    </aside>

    <section class="center">
      <nav class="tabs">
        <TabBar :view="view" :counts="tabCounts" @select="view = $event" />
        <input
          v-if="view === 'items'"
          v-model="query"
          class="search"
          placeholder="Filter items…"
        />
      </nav>

      <div
        v-if="view === 'structure'"
        id="panel-structure"
        class="panel tree"
        role="tabpanel"
        aria-labelledby="tab-structure"
      >
        <pre v-if="revision">{{ renderTree(tree) || "(empty proposal)" }}</pre>
        <p v-else class="muted">Scan a source to see the proposed folder tree.</p>
        <section v-if="snapshot?.directory_roles?.length" class="roles">
          <h2>Folder roles</h2>
          <article v-for="role in snapshot.directory_roles" :key="role.root" class="card">
            <strong>{{ role.root }}</strong>
            <span class="muted">{{ roleKindLabel(role.kind) }} · {{ role.reason }}</span>
          </article>
        </section>
        <section v-if="snapshot?.skipped.length" class="skipped">
          <h2>Skipped</h2>
          <p class="muted">
            {{ snapshot.skipped.length }} entries were not proposed (hidden, junk, symlinks,
            or inside protected projects).
          </p>
          <ul>
            <li v-for="entry in snapshot.skipped" :key="entry.path">
              {{ entry.path }} · {{ skippedReasonLabel(entry) }}
            </li>
          </ul>
        </section>
      </div>

      <div
        v-else-if="view === 'items'"
        id="panel-items"
        class="panel"
        role="tabpanel"
        aria-labelledby="tab-items"
      >
        <table>
          <thead>
            <tr>
              <th>File</th>
              <th>Family</th>
              <th>Destination</th>
              <th>Review</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="row in rows"
              :key="row.type === 'file' ? row.entry.id : row.bundle.id"
              :class="{ selected: row.type === 'file' && selectedAsset === row.entry.id }"
              @click="
                selectedAsset =
                  row.type === 'file' ? row.entry.id : (row.members[0]?.id ?? null)
              "
            >
              <template v-if="row.type === 'file'">
                <td :title="row.entry.path">{{ row.entry.path }}</td>
                <td>{{ familyOf(row.entry) }}</td>
                <td>{{ placementFor(row.entry.id)?.destination ?? "—" }}</td>
                <td>
                  <select
                    :value="placementFor(row.entry.id)?.review ?? 'proposed'"
                    :aria-label="`Review ${row.entry.path}`"
                    @click.stop
                    @change="
                      setReview(
                        row.entry.id,
                        ($event.target as HTMLSelectElement).value as
                          | 'accepted'
                          | 'rejected'
                          | 'proposed',
                      )
                    "
                  >
                    <option value="proposed">Proposed</option>
                    <option value="accepted">Accepted</option>
                    <option value="rejected">Rejected</option>
                  </select>
                </td>
              </template>
              <template v-else>
                <td :title="row.members.map((member) => member.path).join(', ')">
                  {{ row.bundle.label || row.bundle.kind }}
                  <span class="muted">
                    · {{ constraintLabel(row.bundle.constraint) }} ·
                    {{ row.members.map((member) => member.path).join(", ") }}
                  </span>
                </td>
                <td>bundle</td>
                <td>{{ placementFor(row.members[0]?.id ?? "")?.destination ?? "—" }}</td>
                <td>
                  <select
                    :value="placementFor(row.members[0]?.id ?? '')?.review ?? 'proposed'"
                    :aria-label="`Review bundle ${row.bundle.label}`"
                    @click.stop
                    @change="
                      setReviewAssets(
                        row.members.map((member) => member.id),
                        ($event.target as HTMLSelectElement).value as
                          | 'accepted'
                          | 'rejected'
                          | 'proposed',
                      )
                    "
                  >
                    <option value="proposed">Proposed</option>
                    <option value="accepted">Accepted</option>
                    <option value="rejected">Rejected</option>
                  </select>
                </td>
              </template>
            </tr>
          </tbody>
        </table>
      </div>

      <div
        v-else-if="view === 'relationships'"
        id="panel-relationships"
        class="panel"
        role="tabpanel"
        aria-labelledby="tab-relationships"
      >
        <article v-for="bundle in snapshot?.bundles ?? []" :key="bundle.id" class="card">
          <strong>{{ bundle.label || bundle.kind }}</strong>
          <span class="muted">
            {{ constraintLabel(bundle.constraint) }} · {{ bundle.members.length }} members
          </span>
        </article>
        <p v-if="!snapshot?.bundles.length" class="muted">No bundles detected yet.</p>
      </div>

      <div
        v-else
        id="panel-activity"
        class="panel"
        role="tabpanel"
        aria-labelledby="tab-activity"
      >
        <AnalysisStream :stages="analysisStages" :lines="logLines" :progress="progress" />
        <ul v-if="issues.length">
          <li v-for="(issue, index) in issues" :key="index">
            <strong>{{ issue.severity }}</strong> {{ issueLabel(issue) }}
          </li>
        </ul>
        <PreviewDiff
          v-if="plan || journal"
          :rows="diffs"
          :counts="counts"
          :journal-status="journal?.status"
          :dry-run="journal?.dry_run"
        />
        <p v-if="journal" class="muted">
          {{ journal.dry_run ? "Preview journal" : "Apply journal" }}
          {{ journal.status }} · {{ journal.entries.length }} operations
        </p>
        <p v-if="!issues.length && !plan && !journal && !logLines.length && !progress" class="muted">
          Scan progress, validation, and apply results show up here.
        </p>
      </div>

      <footer class="status">
        <div class="status-copy">
          <WorkflowStepper :current="step" />
          <span v-if="busy">
            Working…
            <template v-if="progress">
              {{ progress.stage }} {{ progress.current
              }}<template v-if="progress.total">/{{ progress.total }}</template>
              — {{ progress.message }}
            </template>
          </span>
          <span v-else-if="engineError" class="error">{{ engineError }}</span>
          <span v-else-if="snapshot">
            {{ files.length }} files · {{ snapshot.bundles.length }} bundles ·
            {{ snapshot.projects.length }} projects · {{ snapshot.skipped.length }} skipped
            <template v-if="revision"> · {{ approved }} accepted</template>
          </span>
          <span v-else class="muted">Add a source, pick an intent, then scan.</span>
        </div>
        <div class="actions">
          <button type="button" :disabled="busy || !revision" @click="acceptAll">
            Approve all proposed changes
          </button>
          <button type="button" :disabled="busy || !revision" @click="validatePlan">
            Validate
          </button>
          <button type="button" :disabled="busy || !plan" @click="runApply(true)">
            Preview
          </button>
          <button
            type="button"
            class="primary"
            :disabled="busy || !plan || errorIssues.length > 0"
            @click="requestApply"
          >
            Apply
          </button>
          <button
            type="button"
            :disabled="busy || !journal || journal.dry_run || journal.status === 'undone'"
            @click="runUndo"
          >
            Undo
          </button>
        </div>
      </footer>
    </section>

    <aside class="rail right">
      <h2>Inspector</h2>
      <template v-if="selectedEntry">
        <p><strong>{{ selectedEntry.path }}</strong></p>
        <p class="muted">{{ placementFor(selectedEntry.id)?.rationale }}</p>
        <dl>
          <template v-for="bag in selectedEvidence" :key="bag.asset + bag.source.source">
            <div v-for="(value, key) in bag.facts" :key="key">
              <dt>{{ key }}</dt>
              <dd>{{ value }}</dd>
            </div>
          </template>
        </dl>
      </template>
      <p v-else class="muted">Select an item to inspect evidence.</p>
      <h2>Assistant</h2>
      <div class="chat">
        <p
          v-for="(line, index) in chatLog"
          :key="index"
          :class="line.role"
        >
          {{ line.text }}
        </p>
        <p v-if="!chatLog.length" class="muted">
          Tools can search, inspect bundles, group podcasts, rename from tags, and validate.
        </p>
      </div>
      <form class="row" @submit.prevent="sendChat">
        <input
          v-model="draft"
          :disabled="busy || !revision"
          placeholder="Keep RAW and JPEG pairs together…"
        />
        <button type="submit" :disabled="busy || !revision">Send</button>
      </form>
    </aside>
    <ApplyConfirm
      v-if="confirmApply && snapshot"
      :root="snapshot.root"
      :summary="confirmSummary"
      @cancel="confirmApply = false"
      @confirm="confirmAndApply"
    />
  </div>
</template>
