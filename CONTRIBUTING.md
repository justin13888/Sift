# Contributing to Sift

Thank you for considering a contribution. Read this whole page before opening a pull request: one part of
it is a legal agreement, and a contribution that does not accept it cannot be merged.

For how to work in this repository — the specification, the hard constraints, and the commands — read
[`AGENTS.md`](AGENTS.md) and [`docs/README.md`](docs/README.md).

## Licensing, and why there is an agreement

Sift is published under the [GNU Affero General Public License, version 3](LICENSE). Anyone may use,
study, modify, and redistribute it under those terms.

Sift is also distributed through the Mac App Store, whose terms a copyleft licence does not let a mere
licensee accept. Only a party holding the right to license all of Sift's code under other terms can ship
it there. Today the copyright holder holds that right because they hold all of the copyright, and that
stays true only while every contribution arrives under an agreement preserving it. A contribution that
lands without one cannot be covered afterwards if its author is unreachable or unwilling, so the
agreement is required from the first external contribution onward.

The agreement below is a **licence, not an assignment**: you keep the copyright in your work and may do
anything you like with it elsewhere. It is **asymmetric**, and says so: it lets the copyright holder
distribute your contribution under terms other than the AGPL, including proprietary store terms, while
everyone else — you included, for the combined work — receives it under the AGPL.

The reasoning, and the alternatives that were rejected, are recorded as D-112 in
[`docs/product/platforms-and-distribution.md`](docs/product/platforms-and-distribution.md).

## How to accept the agreement

Every commit in your pull request carries this trailer, on its own line at the end of the commit message:

```
Contributor-Agreement: Sift CLA 1.0
```

The trailer is your acceptance of the version of the agreement it names, for the contribution in that
commit. `git commit --trailer "Contributor-Agreement: Sift CLA 1.0"` adds it. If you are contributing on
behalf of an employer or another organisation that holds rights in your work, the organisation must accept
the agreement too; say so in the pull request before it is reviewed.

Commits authored by the copyright holder do not carry the trailer, because there is nothing to license.

A new version of the agreement applies only to contributions whose trailer names it. It never changes the
terms of a contribution already made.

## Sift Contributor Licence Agreement, version 1.0

This agreement is between **You**, the person or entity submitting a Contribution, and **Justin Chung**
(the **Maintainer**), the copyright holder of Sift, and applies to every Contribution You submit whose
commit carries the trailer naming this version.

1. **Definitions.**
   - **Contribution** means any original work of authorship — including any modification of or addition
     to an existing work — that You intentionally submit to the Maintainer for inclusion in Sift, in any
     form, including a commit, patch, pull request, or issue attachment.
   - **Sift** means the software project maintained by the Maintainer at
     `https://github.com/justin13888/Sift`, together with its documentation and any work derived from it.

2. **Copyright licence.** You grant the Maintainer, and recipients of software distributed by the
   Maintainer, a perpetual, worldwide, non-exclusive, no-charge, royalty-free, irrevocable licence to
   reproduce, prepare derivative works of, publicly display, publicly perform, distribute, and sublicense
   Your Contributions and such derivative works, **under any terms the Maintainer chooses, including terms
   other than the licence under which Sift is published and including proprietary terms**. The Maintainer
   may transfer this licence, in whole, to a successor who takes over the Maintainer's rights in Sift.

3. **Patent licence.** You grant the Maintainer, and recipients of software distributed by the
   Maintainer, a perpetual, worldwide, non-exclusive, no-charge, royalty-free, irrevocable patent licence
   to make, have made, use, offer to sell, sell, import, and otherwise transfer Your Contributions, where
   the licence applies only to patent claims licensable by You that are necessarily infringed by Your
   Contribution alone or by its combination with Sift. If any entity institutes patent litigation alleging
   that Sift or a Contribution constitutes patent infringement, any patent licence granted to that entity
   under this agreement terminates as of the date the litigation is filed.

4. **What You keep.** You retain all right, title, and interest in Your Contributions. Nothing in this
   agreement assigns copyright or restricts Your own use of Your Contributions.

5. **Your representations.** You represent that:
   - each Contribution is Your original creation, or You have the right to submit it under this agreement
     and You identify in the pull request any part of it that is not Your own, with its source and
     licence;
   - You are legally entitled to grant the licences above, and, where Your employer or another party has
     rights in Your Contributions, that party has permitted You to make them on its behalf or has accepted
     this agreement itself;
   - to Your knowledge, no Contribution infringes the rights of any third party.

   You agree to tell the Maintainer if You learn that any of these statements has become inaccurate.

6. **Public availability.** The Maintainer commits that every Contribution accepted into Sift will remain
   available to the public under the GNU Affero General Public License version 3, or under another licence
   approved by the Open Source Initiative, for as long as the Maintainer distributes Sift in any form.

7. **No obligation.** The Maintainer is not obliged to use, merge, or distribute any Contribution.

8. **No warranty.** Unless required by applicable law or agreed in writing, You provide Your
   Contributions on an "as is" basis, without warranties or conditions of any kind, and You are not
   expected to provide support for them.

## Before opening a pull request

- Run `mise run check`, the local sweep to run before pushing.
- Write commits as Conventional Commits, and reference requirements and decisions by identifier.
- Add the agreement trailer to every commit.
