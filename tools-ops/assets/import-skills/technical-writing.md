---
id: technical-writing
abstract: 技术文档写作规范（house style）：结论先行、断言可溯源、一事实一出处、禁用 AI 腔构造清单；含五类文档编辑规则、起草前检查点、句子与词汇纪律、置信标签与文档保鲜 | 适用场景: 撰写或修订任何技术文档（设计文档/README/参考文档/手册/变更日志/事故报告）、交付前的文风终审
---

# 技术文档写作（Technical writing）

**ID**: `technical-writing`

**版本**: 1.0.0

**描述**: 技术文档的统一写作规范（house style）：结构规则、真相与溯源规则、禁用构造清单。核心原则：文档陈述当前、已验证的行为，结论先行，每条断言可溯源，每个事实只有一个出处；单薄的文档胜过夸大的文档。

**状态**: 正式

**来源**: github.com/riekelt/technical-writer（MIT License）；2026-10-11 导入。由原文 SKILL.md、references/style.md、references/truth.md 三个文件合并为单文档（附录 A / 附录 B）。

## 适用场景

- 撰写、重组或修订任何技术文档：设计文档、README、参考文档、计划、报告、变更日志、事故报告
- 需要"经得起匆忙的人读第二遍"的技术正文（规则面向文档，不面向聊天回复）
- 交付前的文风终审：对 AI 生成的长文做去味处理（禁用构造清单见 Hard rules 与 Appendix A）

## 使用说明

- 规则正文为英文，这是原文设计：文档用各自语言撰写与审阅，结构规则与真相规则适用于任何语言；Appendix A 的词汇表为英文特定，其他语言使用自身的词汇体系。
- 中文文档场景：Appendix A 的禁用构造模式按中文等价表述检查（反义对仗、重要性宣告、结尾总结句、AI 腔词汇、中文破折号滥用等）。
- 兄弟技能（writing-design-docs / recording-decisions / writing-changelogs / writing-runbooks / writing-issues / writing-postmortems / documenting-legacy-codebases / diagramming-processes / documenting-contracts / reviewing-technical-prose）为同套件扩展技能；未导入时忽略各表格中的 "Also load" 路由提示，仅按本技能规则执行。
- 平台适配：原文中的 "CLAUDE.md or the platform's equivalent" 对应本平台的项目 agent 指引（AGENTS.md）或项目记忆。

---

以下为技能规则正文（英文原文）。

## Overview

House style for technical documents. The rules are in English; documents keep their own language: a Dutch document is written and reviewed in Dutch. The structural and truth rules apply in any language; the vocabulary lists in Appendix A are English-specific and other languages carry their own. Core principle: **a document states current, verified behavior, conclusion first, with every claim traceable to a source, and every fact living in exactly one place.** A thin document beats an overstated one.

These rules govern documents, not chat replies. Length and section rules apply to running prose, not to reference lists like this file.

## When to invoke

Load this skill for any technical prose, then the matching document-type skill on top. These are routing hints; each skill's own body is the authority.

When a repository regularly produces documents, record the invocation in project memory: one line in its `AGENTS.md` or the platform's equivalent, telling agents to load this skill for any technical prose there.

| The task is | Also load |
|---|---|
| A proposal, RFC, design doc, spec, or migration plan | `writing-design-docs` |
| Recording a decision, an ADR, a decision log entry | `recording-decisions` |
| A changelog entry, release notes, "what shipped" | `writing-changelogs` |
| A runbook, setup guide, procedure, troubleshooting entry | `writing-runbooks` |
| A tracker item: epic, story, task, bug report, spike, acceptance criteria | `writing-issues` |
| A migration or deprecation guide | `writing-runbooks` |
| A postmortem, incident report, root-cause analysis | `writing-postmortems` |
| Documenting an existing, under-documented system; regrounding stale docs against code | `documenting-legacy-codebases` |
| A process, lifecycle, or interaction diagram; drawing business logic | `diagramming-processes` |
| An API reference: endpoints, message payloads, DTOs, CLI commands, file formats, webhooks | `documenting-contracts` |
| Reviewing or rewriting someone else's text; the final pass before delivering any document | `reviewing-technical-prose` |

## Non-goals

- Does not govern marketing copy, social posts, or UI microcopy; only the truth rules in Appendix B still bind those.
- Does not decide content: what is true comes from the sources, not from the style.
- Does not license restyling existing documents that follow their own conventions; apply the precedence rules below.
- Does not yield to a request to make a document "punchy", "compelling", or "persuasive": the numbers persuade, the register stays plain.

## Mandatory checkpoint before drafting

Before drafting a new document or substantial section, derive and state in working notes:

`Kind: <normative|descriptive|historical|runbook|reference> | Audience: <who> | Purpose: <the verb the reader must accomplish> | Non-goals: <what this deliberately does not cover>`

Fill the fields from the request, sources, and repository context. State a safe assumption and continue when it does not materially change the result. Ask only when an unknown would change the audience, substance, or scope. Never invent a fact to complete the checkpoint; do not insert the checkpoint into the finished document unless its schema requires it.

## Read first, then write

Before writing a line:

1. Read up to two comparable documents in the same directory; adopt their structure, tone, and conventions. When fewer than two comparable documents exist, read every available example, then use repository-level conventions. Do not block because the directory is new.
2. Check for a `README.md` that indexes the documents. If it exists, add the new document to it.
3. Check whether the topic already lives somewhere. Extending the owner is almost always better than starting a rival document beside it.

## Rule precedence

When instructions conflict, apply them in this order:

1. Truth, safety, and historical-integrity rules.
2. An explicit exception in the active document-type skill.
3. The shared hard rules below.
4. An explicit schema and the conventions in the target directory.
5. Shared style preferences in Appendix A.

Existing practice controls only choices that a higher rule does not settle. It cannot weaken sourcing, rewrite accepted history, or represent unverified work as shipped. A document-type exception must name the rule it bends and the boundary of the exception.

An author's stated style choice outranks the shared style preferences, and nothing else. When the author says leave my voice alone, the style pass stops; truth, safety, and history stay binding whoever objects.

## Classify the document before editing it

The edit rule differs per kind. "Update docs to match code" is actively wrong for two of the five.

| Kind | Examples | Edit rule |
|---|---|---|
| Normative | architecture principles, contracts, style guides | If code violates the contract, do NOT water the contract down; record the violation. Edit only when the contract itself names deleted or renamed concepts. |
| Descriptive | flows, component docs, state machines | Update to match code exactly. Verify against implementations, not names. |
| Historical | changelogs, old specs, decision logs, applied migrations | Never rewrite history. Flag discrepancies; supersede with a new entry. |
| Runbook | operations, troubleshooting, release procedures | Update to match reality; every step must have been actually run. |
| Reference | API docs, config references, indexes | Exhaustive: every key, every flag, with defaults and a Usage column. |

If an entire document describes something deleted, do not delete the file: mark it obsolete with a one-line banner ("> NOTE: describes removed component X; see Y") and leave the removal to the owner.

## Hard rules

Non-negotiable, in every document:

- **No em dashes, no en dashes, no ` -- ` dashes.** Use commas, colons, semicolons, periods, parentheses, or ` - ` with spaces as an aside marker. A plain hyphen serves ranges (`2026-2030`). Check all three forms before delivery.
- **No changelog section and no "last updated" field inside a document.** This holds per sentence too: describe current behavior, never the previous behavior; "this step replaced the manual check" belongs in a migration doc or release note, not in a procedure.
- **No delivery history as narrative status in prose, comments, names, or strings**: no phases, task IDs, ticket keys, SHAs, or plan references that merely describe how work was delivered. A pinned commit may appear solely as claim evidence. Ticket keys may appear as functional metadata in tracker records, commit messages, planning documents, and citations; never in code comments or user-facing content.
- **Never state as fact what you cannot trace** to code at a cited path, a pinned commit, a test, a document, or a primary source. A plausible guess presented as fact is worse than "the source says nothing about this." See Appendix B.
- **One fact, one home.** Everything else links to the owner. A summary may route, never decide: when an index and its source disagree, the source wins and the index is the bug.
- **Accepted decisions and applied migrations are immutable.** Corrections are new dated entries, never edits to history.
- **Headings in sentence case**, never Title Case.

## Workflow

1. **Read first** (above), and classify the document.
2. **Declare before drafting**: fill the checkpoint above (kind, audience, purpose, non-goals).
3. **Draft conclusion-first** at every level: document, chapter, paragraph. No run-up, no context paragraphs before the outcome. Each chapter opens with what came out of it, not how it was approached.
4. **Ground every claim** and label its confidence: Appendix B.
5. **Style pass** over sentences, words, headings, and the banned-constructions list: Appendix A.
6. **Verify before delivery**: load `reviewing-technical-prose` for the checklist. Check references by actually following them.

## Audience

Know who you write for before starting. Not "users": the actual person, where they are, and the verb they must accomplish.

| Reader | What they need |
|---|---|
| Developers | The why behind the choice, and what changes about their work tomorrow |
| Tech leads / architects | Trade-offs, alternatives, long-term consequences |
| Management / product owner | What it yields, what it costs, which risks |
| External parties | No internal team names or jargon without explanation |

Several groups at once: the summary reads for the broadest group, the rest may deepen. A phrase that would need a footnote needs rewriting, not a footnote.

## Common mistakes

- Starting at the solution. First the problem, then the proposal.
- Documenting what the code already shows. Documents own reasoning, constraints, invariants, and alternatives; the code owns the what.
- Unverifiable claims: "faster", "better" without a number or source. Add the number or cut the claim.
- Filling a gap with a plausible guess instead of naming the gap.
- Vague owners: "this still needs investigation" without a name or role.
- Listing only benefits. Every proposal names its costs.
- Silently trimming, reordering for emphasis, or restyling a host document during an edit pass. Edits are surgical: preserve voice, structure, numbering, and IDs.
- A document past roughly 800 lines of prose: split it and let the main document link to the parts.

## Appendix A: Style (sentences, words, banned constructions)

These rules bind running prose in technical documents. They do not bind quoted material, code blocks, schemas, or an example that discusses a banned phrase as evidence.

### Sentence rules

- One claim per sentence. Average around 20 words, hard maximum 35. In procedures: one action per sentence, present tense or imperative, with a visible actor. No semicolon chains of actions.
- Active voice with a named actor: "the pipeline sets the tag", not "the tag is set". An obligation names its owner: "the team confirms this before phase 1", not "that must be confirmed".
- Give agency to the thing that acted. A framework does not "want", a diagram does not "prove", an architecture does not "decide" unless the implementation literally performs that action. Name the code, test, or person that did it.
- Vary sentence length. Three or more short sentences in a row read as staccato: merge them. When a claim carries a condition, cause, exception, or consequence, keep it in the sentence with the claim rather than stating it as a separate short sentence. The average and the hard maximum still bind, and a second claim still gets its own sentence.
- Do not open three consecutive sentences with the same subject. Merge, switch subject, or lead with the action. One repetition is fine; the pattern is the problem.
- Keep articles. "The context window", never "Context window": dropped articles read as headline voice.
- No noun stacks over three words. "Agent retry budget config" is unreadable.
- Each paragraph should change what the next paragraph can say. If paragraphs can be rearranged without changing the argument, they are separate mini-essays, not one piece of writing. Do not restate a point with new nouns to make the piece feel complete.

### Word choice

- Replace an adjective with a number wherever one exists. "140 keys", not "many keys".
- One term per concept, the whole document through. Pick at first use and never rotate synonyms, including verbs for the same action (create/make/generate). More than five defined terms: add a glossary appendix.
- Label an estimate as an estimate, with what it depends on. A fact read from a repo or API needs no hedge.
- Write "is" and "has" where they fit, not "serves as", "functions as", "acts as", "features", "boasts", "comprises".
- Plain verbs over formal ones: "test", not "exercise" or "trial"; the same for every formal variant of an everyday verb.
- Concrete over abstract: a version number beats "recent releases", a named failure mode beats "issues".
- Metaphor only where it explains something the literal description cannot. In gates and procedures the action wins: "the release stops until the product owner approves", not "the train waits". A domain term that happens to be a metaphor may stay; the decoration around it goes.
- Keep the register steady. A plain engineering account must not turn into a slogan, a sales page, or an academic abstract for one paragraph.

Two self-checks apply to every finished paragraph:

- **The read-aloud rule.** Read the paragraph aloud, and rewrite whatever you would not say to a colleague.
- **The remove-the-name test.** For any text about a specific system (a README opening, an overview, a design doc summary): delete the product name and reread. If a stranger could no longer tell what the text is about, it is generic and carries no information; rewrite from the system's own specifics. Does not apply to reference tables and procedures, which are legitimately generic in shape.

### Headings name the content

Four heading kinds: noun phrase, infinitive or imperative, question, and symptom. Use a noun phrase by default; use an infinitive or imperative above a procedure; use a question only in a genuine FAQ or troubleshooting list; use the verbatim symptom in a troubleshooting entry ("Containers won't start", the verbatim error string), naming what the reader searches for.

Banned: the heading that names the question instead of the content ("What tips the answer", "Why this is important"). A heading states the finding, never promises a reveal. The first sentence under a heading never repeats the heading: under "## Rollback" write the mechanism, not "Rollback is important."

### Banned constructions

Each entry names the pattern; the quoted phrases are examples, and paraphrases of the pattern are equally banned. The patterns are language-agnostic; the example vocabulary is English, and a document in another language gets the same patterns checked against that language's own vocabulary list where one exists.

| Pattern | Example | Repair |
|---|---|---|
| Antithesis / negative parallelism | "This is not a technical choice, it is an organizational one"; "not X but Y"; softened forms "less X than Y" | State the positive claim: "the choice affects the organization more than the technology" |
| The reframe-denial | "The number was never the hard part" | Say what the hard part is, directly |
| Meaning-sentence at the end of a section | "That distinction matters"; a closer explaining why the preceding text was important | Delete it; end on the last fact |
| Manufactured-insight setup | "And the part most people miss:"; "What nobody tells you is" | State the point without announcing its rarity |
| False-candor setup | "Here's the honest catch:"; "To be fair," as a pivot | State the caveat |
| Importance announcement | "What matters is", "The key takeaway is", "It is important to note" | State the concrete consequence |
| Recap sentence | "Overall", "In conclusion", "The bottom line" | End on the last fact |
| Restatement for emphasis | A second sentence repeating the first with more force | Keep the better one |
| Rhetorical triad / rule of three | "faster, safer, and more predictable" | Two items, four items, or the one measured claim: "saves two manual steps per release" |
| Question headings and indirect-question headings | "Why this matters", "What X means", "How does Y work?" | Noun phrase naming the content (see "Headings name the content" above); real questions only in a genuine FAQ |
| Throat clearing | "There are several ways to", "In order to", "This document will explore" | Start with the thing the sentence is about |
| Filler | "concretely", "in practice", "note that", "essentially", "basically", "the core is" | Delete |
| Inflated adjectives | crucial, robust, fundamental, essential, significant, seamless, comprehensive | A number, a named property, or nothing |
| Promotional vocabulary | groundbreaking, state-of-the-art, game-changing, world-class, revolutionary, cutting-edge | Concrete, verifiable statements only |
| Cursed vocabulary | delve, intricate, tapestry, pivotal, underscore, foster, testament, leverage, unlock, realm, nuanced, holistic, "landscape"/"navigate" figuratively | The plain word |
| Trailing participle analysis | ", highlighting the need for", "which underscores that", ", streamlining the process" | Delete the tail; it adds no fact |
| False range | "from architecture to culture" where the ends share no scale | Name the actual items |
| Stacked hedging | "could potentially, in some cases, arguably" | At most one qualifier per claim, and only when the source demands it |
| Vague attribution | "experts say", "research suggests" | Name the source and the finding, or own the statement as opinion |
| Symbolic gloss | "This represents a shift toward" | Show the consequence that makes the interpretation true |
| Bold-lead bullets | "**Performance:** the system..." repeated down a list | Bullets that need headings are subsections; a list needs parallel items |
| Emoji furniture | emoji as bullets, prefixes, or category markers | Plain markers |
| Assistant residue | "Here is an overview", "I hope this helps", "Great question", reasoning traces, placeholder text, leaked citation markers | Delete; never ship correspondence as prose |
| Production residue | "as requested", "per the instructions", "unlike the previous version", "this guide deliberately avoids X", "this section was added because" | A constraint shapes the design and stays invisible; exclusions are stated only when the audience needs them |
| Leaked intent example | an example the requester gave to communicate intent, appearing as content in the deliverable | Requester examples are diagnostic material: read the audience and abstraction level from them; the document carries its own examples |
| Closing offer | ending a document with a question or an offer to the reader | Documents end on content; an offer belongs in chat, one line at most |

### Formatting

Formatting reveals structure already present in the material; it must not supply importance the prose has not earned.

- Tables only when items carry two or more properties each and a reader compares across them. A two-column table of prose is prose, or a list. Structure earns its place when a consumer acts on it; otherwise it buys indentation.
- Do not put a heading above every paragraph, or turn a list into a miniature article.
- No section shorter than three paragraphs in running analysis; shorter means it is not a section. Outside running analysis a heading still needs more than one paragraph under it. Template-mandated sections (a summary, a status table, a definitions block) are exempt: they are as long as their job requires.
- Structural Markdown in specifications, provenance tables, and required review formats is functional, not decorative. These rules never flatten a document whose schema carries evidence.

## Appendix B: Truth (claims, sourcing, and staleness)

### Grounding claims

- Every load-bearing claim traces to a source stated in the document: a `file:line`, a pinned commit, a test, an exact query, or a primary source. If a claim is not backed, say so in the document; "I read this from empty files" is a stronger sentence than a confident guess.
- When facts are handed to you without sources, never invent a citation and never block: use the fact, mark it `**[source wanted: <what and from whom>]**`, and continue. The owner fills the gap before the document is final.
- Within one document, one section owns the numbers, usually the grounding or summary section. A decision box or later chapter cites that section (`ch. 1`) or restates a derived form ("drops by more than half"), never a second copy of the raw figure.
- Ground in the real artifact, not in another document's summary of it. A document's own status header is not evidence of anything; verify against the code or history before building on it.
- Print the command behind a count, and run it against committed state (HEAD), never the working tree. Say when a number was checked and that numbers drift.
- Prefer primary sources. When official sources conflict, document the conflict and state which source wins and why; never silently pick the favorable value.
- Unknowns stay explicitly unknown: "not stated by the vendor" means no public statement was found, not that the thing is absent. Define the placeholder's meaning for the reader.
- Third-party behavior is perishable: any vendor, model, API, or library claim carries the date and commit at which it was observed. Prices, versions, and limits are always dated, never stated as permanent.
- Date a claim once, next to the claim, not as freshness copy sprinkled through the document.

### Confidence labels

When a document mixes evidence grades, define the tiers in a "How to read this" section and use them consistently:

- **Measured**: reproduced from the repo or a script named in the document.
- **Sourced**: traced to a named primary source, cited where the claim is made.
- **Estimated**: model or judgment output, labeled as such with what it depends on.

Never claim a state you have not verified. Distinguish three states: written (exists in the repo), shipped (live), and externally verified (checked in the external system). "Verified" always names what was checked: "PASS (checked X and Y)", never a bare checkmark.

### Fact versus proposal versus hypothesis

- Analysis is factual and verifiable; phasing and recommendations are not. Say explicitly which part is which.
- Keep hypotheses labeled as hypotheses until repeated evidence supports them; keep the interpretation in a separate column or sentence so it cannot become evidence by repetition.
- Carry known weaknesses forward instead of quietly dropping them. A deliberate limitation has a known cost; state that cost.
- In a document that circulates, name the role or the process failure, never the person.

### Reviewed documents

When a document requires approval, keep a dated sign-off log: `- YYYY-MM-DD <reviewer>: approved <scope>`, one line per review event. A document that changed after its latest sign-off counts as unreviewed; where tooling exists, fail the gate when content is newer than the last sign-off line.

This is a named exception to the no-changelog rule, allowed because approval events are not derivable from git history: the log records reviews, never edits. It lives beside the document or in a companion file, and it stays append-only.

### Keeping documents true

- Documentation updates in the same change as the thing it describes. Documentation is part of done, not a follow-up ticket.
- When a fact changes, sweep every surface that states it. Enumerate the affected pages, fix them all, and scope the sweep precisely so it does not become a rewrite.
- Correct visibly. Say what you corrected rather than quietly rewriting it. Delete only what is wrong; age alone is not a reason.
- Superseded documents get a banner, not deletion: what superseded it, where authority moved, and why the file still exists. A decommissioned document becomes a pointer ("the backlog now lives in X; do not re-add items here") with a note where the old content went.
- Surface staleness as a visible note instead of leaving a wrong picture in place.
- Retired false claims get a regression check (a grep in CI or a checklist line) so they cannot silently come back.
- Indexes are derived artifacts. Rebuild them from the leaves and verify counts against the actual files; every entry gets a one-line purpose.
- A descriptive document names the code it describes. Put the path (a module, a directory, an entry point) in the front matter or the opening, so drift checks have an anchor. A path is not a date; this does not violate the ban on "last updated" fields.
- When auditing documentation, count documented items against total public items (endpoints, commands, config keys) rather than judging completeness by impression.
- Diagrams are source-controlled text (PlantUML, for instance) and rendered output is derived. Delete stale renders rather than letting them mislead, and note which sources await re-rendering. Everything legible in an image is a claim, subject to the same rules as prose.
- Tense must match status. A dormant system cannot take the present tense; an unreleased one cannot imply availability.
- "was refactored to", "now uses", and "replaces the old" describe an edit, not the system. Explain what the system does; keep the diff only where the history itself is the evidence (migration docs, release notes).

### Rules about rules

When a conventions document accumulates its own rules:

- A new rule records the incident that created it (trigger, instruction, added-after, example).
- Add a rule when the same mistake happens twice; once is learning.
- Refine a rule that keeps triggering (sharper trigger, clearer instruction, an example); retire a rule whose underlying cause is fixed. "Be careful" is not a rule.
- Exceptions live next to the rule they bend, not only in the artifact that needed them.
- Automate only the mechanically decidable rules (grep for banned dashes, banned phrases, ticket keys), and mask code blocks and quotations so a document explaining a banned phrase does not fail on its own example. State where the linter stops and judgment starts.
