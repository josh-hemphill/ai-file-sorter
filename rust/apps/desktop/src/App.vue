<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import {
  applyPlan,
  chatRevision,
  connectEngine,
  onEngineProgress,
  patchRevision,
  pickFolder,
  planRevision,
  proposeSession,
  scanRoot,
  undoJournal,
} from "./engine";
import { destinationTree } from "./tree";
import type {
  ApplyJournal,
  CenterView,
  ChatLine,
  IntentPreset,
  ObservedEntry,
  OperationPlan,
  PlanIssue,
  ProgressEvent,
  ProposalRevision,
  WorkspaceSnapshot,
} from "./types";

const engineReady = ref(false);
const engineError = ref<string | null>(null);
const busy = ref(false);
const progress = ref<ProgressEvent | null>(null);
const rootPath = ref("");
const recentRoots = ref<string[]>([]);
const preset = ref<IntentPreset>("inbox");
const view = ref<CenterView>("structure");
const snapshot = ref<WorkspaceSnapshot | null>(null);
const revision = ref<ProposalRevision | null>(null);
const plan = ref<OperationPlan | null>(null);
const issues = ref<PlanIssue[]>([]);
const journal = ref<ApplyJournal | null>(null);
const selectedAsset = ref<string | null>(null);
const query = ref("");
const draft = ref("");
const chatLog = ref<ChatLine[]>([]);

const presets: { id: IntentPreset; label: string; hint: string }[] = [
  { id: "inbox", label: "Tidy inbox", hint: "Broad folders, protect projects" },
  { id: "archive", label: "Build archive", hint: "Refined folders, date prefixes" },
  { id: "media", label: "Media library", hint: "Artist/album names from tags" },
  { id: "custom", label: "Custom", hint: "Same defaults, tweak later in settings" },
];

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

function placementFor(asset: string) {
  return revision.value?.placements[asset];
}

function rememberRoot(path: string) {
  recentRoots.value = [path, ...recentRoots.value.filter((item) => item !== path)].slice(
    0,
    8,
  );
}

async function chooseFolder() {
  engineError.value = null;
  const picked = await pickFolder();
  if (picked) {
    rootPath.value = picked;
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
  try {
    await connectEngine();
    engineReady.value = true;
    const next = await scanRoot(rootPath.value, preset.value);
    snapshot.value = next;
    rememberRoot(rootPath.value);
    const proposed = await proposeSession(next.session, preset.value);
    revision.value = proposed;
    view.value = "structure";
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
    progress.value = null;
  }
}

async function setReview(asset: string, review: "accepted" | "rejected" | "proposed") {
  if (!snapshot.value || !revision.value) {
    return;
  }
  const op =
    review === "accepted"
      ? "accept"
      : review === "rejected"
        ? "reject"
        : "reopen";
  busy.value = true;
  try {
    revision.value = await patchRevision(
      snapshot.value.session,
      revision.value.id,
      `${op} ${asset}`,
      [{ op, assets: [asset] }],
    );
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
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
  } catch (error) {
    engineError.value = String(error);
  } finally {
    busy.value = false;
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
    progress.value = null;
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
onMounted(async () => {
  stopProgress = await onEngineProgress((event) => {
    progress.value = event;
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
});

function familyOf(entry: ObservedEntry): string {
  return entry.family.replace(/_/g, " ");
}
</script>

<template>
  <div class="shell">
    <aside class="rail left">
      <header class="brand">
        <strong>AI File Sorter</strong>
        <span class="muted">{{ engineReady ? "engine ready" : "engine offline" }}</span>
      </header>
      <label class="field">
        Source
        <div class="row">
          <input v-model="rootPath" placeholder="/path/to/folder" />
          <button type="button" @click="chooseFolder">Browse</button>
        </div>
      </label>
      <p class="label">Intent</p>
      <button
        v-for="item in presets"
        :key="item.id"
        type="button"
        class="preset"
        :class="{ active: preset === item.id }"
        @click="preset = item.id"
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
        @click="rootPath = item"
      >
        {{ item }}
      </button>
      <button class="primary" type="button" :disabled="busy" @click="runScan">
        Scan
      </button>
    </aside>

    <section class="center">
      <nav class="tabs">
        <button
          v-for="tab in ['structure', 'items', 'relationships', 'activity']"
          :key="tab"
          type="button"
          :class="{ active: view === tab }"
          @click="view = tab as CenterView"
        >
          {{ tab }}
        </button>
        <input v-model="query" class="search" placeholder="Filter items…" />
      </nav>

      <div v-if="view === 'structure'" class="panel tree">
        <pre v-if="revision">{{ renderTree(tree) || "(empty proposal)" }}</pre>
        <p v-else class="muted">Scan a source to see the proposed folder tree.</p>
      </div>

      <div v-else-if="view === 'items'" class="panel">
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
              v-for="entry in filteredFiles"
              :key="entry.id"
              :class="{ selected: selectedAsset === entry.id }"
              @click="selectedAsset = entry.id"
            >
              <td>{{ entry.path }}</td>
              <td>{{ familyOf(entry) }}</td>
              <td>{{ placementFor(entry.id)?.destination ?? "—" }}</td>
              <td>
                <select
                  :value="placementFor(entry.id)?.review ?? 'proposed'"
                  @change="
                    setReview(
                      entry.id,
                      ($event.target as HTMLSelectElement).value as
                        | 'accepted'
                        | 'rejected'
                        | 'proposed',
                    )
                  "
                >
                  <option value="proposed">proposed</option>
                  <option value="accepted">accepted</option>
                  <option value="rejected">rejected</option>
                </select>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <div v-else-if="view === 'relationships'" class="panel">
        <article v-for="bundle in snapshot?.bundles ?? []" :key="bundle.id" class="card">
          <strong>{{ bundle.label || bundle.kind }}</strong>
          <span class="muted">{{ bundle.constraint }} · {{ bundle.members.length }} members</span>
        </article>
        <p v-if="!snapshot?.bundles.length" class="muted">No bundles detected yet.</p>
      </div>

      <div v-else class="panel">
        <p v-if="progress">
          {{ progress.stage }} {{ progress.current
          }}<template v-if="progress.total">/{{ progress.total }}</template>
          — {{ progress.message }}
        </p>
        <ul>
          <li v-for="(issue, index) in issues" :key="index">
            <strong>{{ issue.severity }}</strong> {{ issue.code }}: {{ issue.message }}
          </li>
        </ul>
        <p v-if="journal">
          Journal {{ journal.id }} · {{ journal.status }} · dry_run={{ journal.dry_run }}
        </p>
        <p v-if="!issues.length && !journal" class="muted">
          Validation and apply results show up here.
        </p>
      </div>

      <footer class="status">
        <span v-if="busy">Working…</span>
        <span v-else-if="engineError" class="error">{{ engineError }}</span>
        <span v-else-if="snapshot">
          {{ files.length }} files · {{ snapshot.bundles.length }} bundles ·
          {{ snapshot.projects.length }} projects
        </span>
        <span v-else class="muted">Add a source, pick an intent, then scan.</span>
        <div class="actions">
          <button type="button" :disabled="busy || !revision" @click="acceptAll">
            Accept all
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
            @click="runApply(false)"
          >
            Apply
          </button>
          <button type="button" :disabled="busy || !journal" @click="runUndo">Undo</button>
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
  </div>
</template>
