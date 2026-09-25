# Relevance corpus

How the [relevance corpus](reference-environment.md#reference-corpus) is recorded, what it holds, and what
the ranking report read over it can and cannot settle.

**Owns:** nothing. This is the protocol for a corpus the [reference environment](reference-environment.md)
requires; the decisions it serves are [D-5 and D-79](../storage/search.md).

## What a judgement is

One judgement is **a query a person actually issued against their own mailbox, and the one message they
were looking for when they issued it.** The corpus is a set of these, against one known container.

The rules follow from what the corpus is for — measuring whether the right message comes back first:

- **Recorded in the moment.** The query is the one typed while looking for something, before the result
  list was seen. A query reconstructed afterwards, or written to find a message already chosen, is a
  query that knows its answer — the same defect that makes a synthetic corpus measure its generator.
- **Verbatim.** The query is kept as typed, operators, typos and all. Correcting it records a query nobody
  issued.
- **One message sought.** Where the person wanted several, the judgement names the one they would have
  opened first. A query that sought nothing in particular is not a judgement.
- **Kept when it failed.** A query whose sought message did not come back, or came back far down, is the
  most valuable judgement in the set. It is recorded by naming the message once found some other way.

## What a judgement holds

The account it was recorded in, the sought message's **provider identifier** and its **internet message
identifier** where it has one, and the query.

It does not hold the local identity. That identity is minted at ingest and replaced by removing and
re-adding an account, so a corpus keyed on it would stop resolving silently — every judgement reading as
"not returned", which is indistinguishable from a ranking failure. The provider identifier is tried first
because it is unique within an account; the internet message identifier second, because
[R-5](../open-questions.md) records that it is not. A message carrying neither cannot be recorded.

A judgement whose message is no longer in the container — deleted since, or its account not present — is
**unresolved**, and is reported apart from the ranking figures rather than counted as a miss.

## Where it lives

**Outside the source tree, readable by its owner only, and never committed.** It is somebody's real
queries keyed on the identifiers of their real mail. The scrubbing [D-115](reference-environment.md)
applies to captured fidelity messages does not transfer: a scrubbed query no longer finds its message, so
the corpus cannot be made shareable without destroying it. Its figures are shared; it is not. The
repository ignores the corpus's conventional file name, as a net under one recorded inside the tree by
mistake.

## Size

A first corpus of **at least fifty judgements**, across **at least two accounts** — a hypothesis, like
every number here. The floor on judgements is set where a single judgement moving changes the reciprocal
rank figure by a few hundredths rather than by a tenth. The two accounts are because D-79's trade is
cross-account coherence against within-account quality, and a corpus of one account cannot see the side
of that trade it paid for.

The set should hold every shape of query a person uses: plain words, a remembered phrase, operators alone,
and operators with words. A query of operators alone carries no relevance signal and is ordered by list
order under D-79; it is kept, because it measures that fallback.

## What is reported

For every resolved judgement, where the sought message landed under **two orders of the same result
set**: D-79's feature ranking, and [D-55](../architecture/presentation-layer.md)'s list order — the order D-79 falls
back to, and so the baseline it has to beat to be worth its cost. Over the set, for each order: mean
reciprocal rank, how many landed first, in the first three, and in the first ten, and how many the query
did not return at all.

A message the query did not return is a **recall failure**, the same under every order, and says nothing
about ranking. It is counted apart for that reason.

## What it can and cannot settle

- **It is a figure, never a gate.** Nothing fails when a number is poor. What counts as the "measured
  ranking failure" D-5 names is a judgement over these figures, and no threshold is recorded for it.
- **It is provisional while message bodies are unindexed.** The features are computed over the sender,
  subject and snippet the store holds; a body match is a snippet match until the synchronization path
  populates the full-text index. A ranking failure measured now may be a coverage failure.
- **It cannot yet compare D-79 with per-account ranking.** D-79's retreat — reconsidering D-6's
  per-account databases — is argued by showing the merged order is materially worse than each index's own.
  That comparison needs the index's own scores, which exist only once the index is populated. Until then
  the report compares D-79 with list order, which is the comparison the product currently ships.
