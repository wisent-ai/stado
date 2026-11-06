# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.69 – 0.23.70](changelog/0.23.69-0.23.70.md)
- [0.23.68 (part 1 of 10)](changelog/0.23.68-part-1.md)
- [0.23.68 (part 2 of 10)](changelog/0.23.68-part-2.md)
- [0.23.68 (part 3 of 10)](changelog/0.23.68-part-3.md)
- [0.23.68 (part 4 of 10)](changelog/0.23.68-part-4.md)
- [0.23.68 (part 5 of 10)](changelog/0.23.68-part-5.md)
- [0.23.68 (part 6 of 10)](changelog/0.23.68-part-6.md)
- [0.23.68 (part 7 of 10)](changelog/0.23.68-part-7.md)
- [0.23.68 (part 8 of 10)](changelog/0.23.68-part-8.md)
- [0.23.68 (part 9 of 10)](changelog/0.23.68-part-9.md)
- [0.23.68 (part 10 of 10)](changelog/0.23.68-part-10.md)
- [0.23.67](changelog/0.23.67.md)
- [0.23.66 (part 1 of 2)](changelog/0.23.66-part-1.md)
- [0.23.66 (part 2 of 2)](changelog/0.23.66-part-2.md)
- [0.23.42 – 0.23.65](changelog/0.23.42-0.23.65.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

### Fixed

- **Azure VM creation is decided by ARM's `error.code`, not by words in its messages:** a NIC or
  VM create refusal was classified by searching the message for `QuotaExceeded`,
  `OperationNotAllowed` or `SkuNotAvailable`, so a message that quoted one of them for another
  reason skipped a location and one that phrased it differently did not. A refusal ARM states is
  now kept with its HTTP status and `error.code`, from the response and from a long-running
  operation that ends Failed or Canceled, and a location is skipped only for those codes. Any other
  refusal is no longer dropped: when no location places the VM, the create fails naming each
  location's refusal. A NIC that cannot be removed after a refused VM, or after a delete, fails the
  call naming the VM, the NIC and both refusals instead of being logged and left holding the name
  the next attempt needs.

### Added

- **The Azure cloud is declared, not built in:** the Resource Manager, the OAuth authority, the
  managed-identity endpoint and the Retail Prices API were constants of the public cloud,
  scattered through VM dispatch, the credit gate, sign-in, role repair, billing, the SKU catalog
  and quota replies. They are now the compute bindings `azure.cloud.resource_manager`,
  `azure.cloud.authority`, `azure.cloud.identity_endpoint` and `azure.cloud.retail_prices`
  (`AZURE_RESOURCE_MANAGER`, `AZURE_AUTHORITY`, `AZURE_IDENTITY_ENDPOINT`, `AZURE_RETAIL_PRICES`),
  each the public cloud's when undeclared, so a deployment can rent from a sovereign cloud and a
  listener on a loopback address can stand in for Azure. A declared endpoint that is neither
  HTTPS nor on a loopback or link-local address refuses every Azure call naming the binding, and
  `stado config show` names the endpoints in force. The ARM client no longer has an
  unauthenticated mode: every request carries the token.
