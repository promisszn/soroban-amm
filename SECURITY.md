# Security Policy

## Supported Versions

Only the latest released version of each contract receives security fixes. Older deployments are not patched — users should migrate to the latest version.

| Contract | Supported |
|---|---|
| `amm` (latest) | ✅ |
| `token` (latest) | ✅ |
| `factory` (latest) | ✅ |
| Any prior version | ❌ |

---

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Please report vulnerabilities privately using one of the following methods:

- **GitHub Private Advisory** (preferred): [Submit a draft security advisory](https://github.com/promisszn/soroban-amm/security/advisories/new) via the GitHub Security tab.
- **Email**: Send a detailed report to the maintainers. Include `[SECURITY]` in the subject line.

### What to Include

To help us triage and reproduce the issue quickly, please provide:

- A clear description of the vulnerability and its potential impact
- The affected contract(s) and function(s)
- Step-by-step reproduction instructions or a proof-of-concept
- Any relevant transaction IDs, test cases, or code snippets
- Your suggested severity (critical / high / medium / low)

---

## What to Expect

| Timeline | Action |
|---|---|
| **Within 48 hours** | Acknowledgement of your report |
| **Within 7 days** | Initial assessment and severity classification |
| **Within 30 days** | Patch development and coordinated disclosure plan |
| **At disclosure** | Public advisory published; reporter credited (if desired) |

We follow a **coordinated disclosure** model. We ask that you give us a reasonable window to patch before publishing details publicly. We will keep you informed throughout the process.

---

## Out of Scope

The following are generally considered out of scope:

- Theoretical vulnerabilities with no practical exploit path on Stellar mainnet
- Issues requiring the attacker to already control the contract admin key
- Denial-of-service via resource exhaustion that only affects the reporter's own account
- Bugs in third-party dependencies (report those upstream)
- Issues in example/client code under `examples/` that do not affect on-chain contracts
- Network-level attacks outside the Soroban / Stellar protocol

---

## Disclosure Policy

- Reporters who responsibly disclose valid vulnerabilities will be credited in the security advisory (unless they prefer to remain anonymous).
- We do not currently operate a bug bounty program, but we deeply appreciate responsible disclosure for a project handling real assets.
