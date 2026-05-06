const form = document.querySelector("#extract-form");
const statusNode = document.querySelector("#status");
const sampleButton = document.querySelector("#sample-button");
const clearButton = document.querySelector("#clear-button");
const submitButton = form.querySelector("button[type='submit']");
const rawJson = document.querySelector("#raw-json");
const matchesBody = document.querySelector("#matches-body");
const suppressedBody = document.querySelector("#suppressed-body");
const matchCount = document.querySelector("#match-count");
const suppressedCount = document.querySelector("#suppressed-count");
const metrics = document.querySelector("#metrics");

const fields = {
  noteId: document.querySelector("#note-id"),
  refsetId: document.querySelector("#refset-id"),
  history: document.querySelector("#history"),
  objective: document.querySelector("#objective"),
  assessment: document.querySelector("#assessment"),
  plan: document.querySelector("#plan"),
  includeSuppressed: document.querySelector("#include-suppressed")
};

const sample = {
  history: "No chest pain. Has cough but denies asthma. Father had diabetes.",
  objective: "Temperature normal. Respiratory rate 18.",
  assessment: "Chest pain.",
  plan: "Screen for depression. Safety net if chest pain worsens."
};

sampleButton.addEventListener("click", () => {
  fields.history.value = sample.history;
  fields.objective.value = sample.objective;
  fields.assessment.value = sample.assessment;
  fields.plan.value = sample.plan;
  fields.includeSuppressed.checked = true;
});

clearButton.addEventListener("click", () => {
  fields.history.value = "";
  fields.objective.value = "";
  fields.assessment.value = "";
  fields.plan.value = "";
  rawJson.textContent = "";
  renderRows(matchesBody, [], renderMatchRow, "No matches");
  renderRows(suppressedBody, [], renderSuppressedRow, "No suppressed matches");
  setMetrics(0, 0, 0);
  setStatus("Ready", "ok");
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  setBusy(true);
  setStatus("Running", "");

  const request = {
    note_id: cleanOptional(fields.noteId.value),
    history: fields.history.value,
    objective: fields.objective.value,
    assessment: fields.assessment.value,
    plan: fields.plan.value,
    include_suppressed: fields.includeSuppressed.checked,
    refset_id: cleanOptional(fields.refsetId.value)
  };

  try {
    const response = await fetch("/v1/extract", {
      method: "POST",
      headers: {
        "Content-Type": "application/json"
      },
      body: JSON.stringify(request)
    });

    const text = await response.text();
    if (!response.ok) {
      throw new Error(text || `HTTP ${response.status}`);
    }

    const payload = JSON.parse(text);
    render(payload);
    setStatus("Complete", "ok");
  } catch (error) {
    setStatus(error.message, "error");
  } finally {
    setBusy(false);
  }
});

function render(payload) {
  const matches = payload.matches || [];
  const suppressed = payload.suppressed || [];

  renderRows(matchesBody, matches, renderMatchRow, "No matches");
  renderRows(suppressedBody, suppressed, renderSuppressedRow, "No suppressed matches");
  setMetrics(matches.length, suppressed.length, payload.elapsed_micros || 0);
  rawJson.textContent = JSON.stringify(payload, null, 2);
}

function renderRows(body, rows, renderer, emptyText) {
  body.replaceChildren();
  if (!rows.length) {
    const row = document.createElement("tr");
    const cell = document.createElement("td");
    cell.className = "empty";
    cell.colSpan = 5;
    cell.textContent = emptyText;
    row.append(cell);
    body.append(row);
    return;
  }

  for (const item of rows) {
    body.append(renderer(item));
  }
}

function renderMatchRow(item) {
  const row = document.createElement("tr");
  addCell(row, item.field);
  addCell(row, item.concept_id);
  addCell(row, item.preferred_term);
  addCell(row, item.matched_text, "matched");
  addEvidenceCell(row, item);
  return row;
}

function renderSuppressedRow(item) {
  const row = document.createElement("tr");
  addCell(row, item.field);
  addCell(row, item.concept_id);
  addCell(row, item.preferred_term);
  addCell(row, item.assertion, "suppressed-tag");
  addEvidenceCell(row, item);
  return row;
}

function addCell(row, value, className) {
  const cell = document.createElement("td");
  if (className) {
    const span = document.createElement("span");
    span.className = className;
    span.textContent = value || "";
    cell.append(span);
  } else {
    cell.textContent = value || "";
  }
  row.append(cell);
}

function addEvidenceCell(row, item) {
  const cell = document.createElement("td");
  const evidence = document.createElement("span");
  evidence.className = "evidence";
  evidence.textContent = item.explanation || "";
  const rules = document.createElement("span");
  rules.className = "rules";
  rules.textContent = (item.rule_ids || []).join(", ");
  cell.append(evidence, rules);
  row.append(cell);
}

function setMetrics(matches, suppressed, micros) {
  matchCount.textContent = matches;
  suppressedCount.textContent = suppressed;
  const values = metrics.querySelectorAll(".metric-value");
  values[0].textContent = String(matches);
  values[1].textContent = String(suppressed);
  values[2].textContent = `${micros} us`;
}

function setStatus(message, kind) {
  statusNode.textContent = message;
  statusNode.className = `status ${kind || ""}`.trim();
}

function setBusy(isBusy) {
  submitButton.disabled = isBusy;
  submitButton.textContent = isBusy ? "Running" : "Run";
}

function cleanOptional(value) {
  const trimmed = value.trim();
  return trimmed.length ? trimmed : null;
}

sampleButton.click();
