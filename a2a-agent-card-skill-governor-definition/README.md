# "a2a-agent-card-skill-governor" Policy Definition

This is the policy-definition half of the **A2A Agent Card Skill Governor** policy. It declares the GCL schema (`gcl.yaml`) and the Anypoint Exchange asset coordinates (`exchange.json`) that the implementation half (`../a2a-agent-card-skill-governor-flex/`) compiles against.

For the full policy behavior, configuration reference, and worked examples, see the [root README](../README.md).

The policy was created with the Omni Gateway Policy Development Kit (PDK). For the complete PDK documentation, see [PDK Overview](https://docs.mulesoft.com/pdk/latest/policies-pdk-overview).

## Schema summary

`gcl.yaml` declares this policy as an **outbound-injection** extension (`metadata/capabilities/injectionPoint: outbound`) for A2A APIs (`metadata/capabilities/assetTypes: a2a,a2av1`), category `security`. The configuration properties are:

- `scopeClaimKey` (string, default `"scope"`) — the `Authentication` custom-property key carrying the caller's scope(s).
- `defaultAllow` (boolean, default `true`) — visibility posture for a skill that matches no `visibility` rule.
- `visibility[]` — ordered allow/deny rules; each has `effect` (required), `audienceType`, `audienceValue`, `skillId`, `skillIdPattern`.
- `skills[]` — upsert entries; each has `audienceType`, `audienceValue`, and a required `skill` object keyed by `skill.id`.

No property uses `format: dataweave` — matching is against the `Authentication` injectable, not payload selectors — and there are no object-literal defaults. The `description` label is kept short (≤256 chars, an Exchange publish limit); the long prose lives in the root README.

## Make command reference

This project has a Makefile that includes the goals used during the definition development lifecycle.

*For more information about the Makefile, see [Makefile](https://docs.mulesoft.com/pdk/latest/policies-pdk-create-project#makefile).*

### Build
The `make build` goal compiles the definition of the policy.

*For more information about `make build`, see [Compiling Custom Policies](https://docs.mulesoft.com/pdk/latest/policies-pdk-compile-policies).*

### Publish
The `make publish` goal publishes the policy definition asset in Anypoint Exchange, in your configured Organization.

Since the publish goal is intended to publish a policy asset in development, the _assetId_ and name published will explicitly say `dev`, and the versions published will include a timestamp at the end of the version. Eg.
- groupId: your configured organization id
- visible name: _{Your policy name} Dev_
- assetId: _{your-policy-asset-id}-dev_
- version: _{your-policy-version}-20230618115723_

*For more information about publishing policies, see [Uploading Custom Policies to Exchange](https://docs.mulesoft.com/pdk/latest/policies-pdk-publish-policies).*

### Release
The `make release` goal also publishes the policy definition to Anypoint Exchange, but as a ready for production asset. In this case, the groupId, visible name, assetId and version will be the ones defined in the project.

*For more information about releasing policies, see [Uploading Custom Policies to Exchange](https://docs.mulesoft.com/pdk/latest/policies-pdk-publish-policies).*

### Release Local
The `make release-local` goal publishes the policy definition as a release asset to the local Anypoint Exchange cache, so you can override it. This target is useful if you are also developing the policy implementation.

*For more information about releasing policies, see [Uploading Custom Policies to Exchange](https://docs.mulesoft.com/pdk/latest/policies-pdk-publish-policies).*

### Policy Examples

The PDK provides a set of example policy projects to get started creating policies and using the PDK features. To learn more about these examples see [Custom policy Examples](https://docs.mulesoft.com/pdk/latest/policies-pdk-policy-templates).
