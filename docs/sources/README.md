# Source evaluation records

Required by `docs/phase-2-design.md` ("Create a dated source-evaluation record for each
candidate"). One file per candidate. **Evaluated on 2026-10-27.** Terms change; re-verify the
exact documents immediately before any integration (robots permission is never authorization).

## Result

| Candidate | Automated collection permitted? | Read API / feed for third parties? | Decision |
|---|---|---|---|
| [62imoveis](62imoveis.md) | **No.** Terms expressly forbid screen/database scraping "para uso comercial ou não" and reproduction without written consent. `robots.txt` alone would allow listing pages, which is exactly the case where robots is not authorization. | None found | **Do not integrate.** |
| [OLX](olx.md) | **No** without prior written authorization: terms forbid "web crawling no Site". Edge returns HTTP 403 to automated clients (even for `robots.txt`). | None (the developer portal is for advertisers sending feeds) | **Do not integrate.** |
| [ZAP Imóveis](zap-imoveis.md) | **No** without authorization: terms forbid using robots to reproduce/copy platform elements and reusing ads/texts/images elsewhere. Edge returns HTTP 403 for `robots.txt`. | None (same Grupo OLX advertiser portal) | **Do not integrate.** |
| [Local brokerages](local-brokerages.md) | **Not established.** No brokerage has been identified; web search was unavailable in this session. | Unknown | **Needs input from the project owner.** |

**0 of the 2 required real adapters qualify today.** Under the plan's own rule: report the gap, build
the source-independent infrastructure, do not represent Phase 2 as complete, and do not substitute a
mock for a real integration.

## What would unblock a real adapter (options for the project owner)

1. **Written authorization** from a portal (OLX / ZAP / 62imoveis) for this specific use, or a
   licensed data/feed agreement. Nothing in this repository contacts anyone; that is the owner's call.
2. **Brokerages that explicitly permit it**: name candidate brokerage sites; each needs the same
   dated review (terms, robots, any public feed/API, rendering, rate limits, image rights).
3. **User-assisted import** (a person reads a listing in a browser and pastes its URL/text or
   imports a file they legitimately hold). This is a human-in-the-loop mechanism rather than automated
   collection, and it would also provide the real, owner-authorized data the price calibration
   needs. It is **not built**: the plan does not include it and it would be an owner decision to add
   it and to decide whether it counts toward the "two real sources" criterion.

## Method (applies to every record)

* Every request used an honest user agent
  (`LooseCanon-source-evaluation/0.1 (manual terms review; contact via repository owner)`), one
  request per document, no retries, no circumvention of any block.
* Only legal/policy pages, `robots.txt` and public developer documentation were requested.
  **No listing or search-result page was requested from any source.**
* A refusal (HTTP 403) was recorded as a finding and not worked around.
* Terms were read from the published documents (PDF text extracted locally). PDF extraction can
  scramble the order of two-column text, so clauses are quoted by wording and the original document
  hash is recorded for later verification. The documents themselves are not stored in the
  repository (copyright); re-download from the URLs.
* The web search and page-summarising tools were unavailable during this session; everything above
  was obtained by direct requests.
