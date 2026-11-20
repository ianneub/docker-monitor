---
name: canary-test
description: Prove the deployed docker-monitor stops over-limit tasks by running a throwaway labelled ECS service on one production host, then tearing it down. Use after rolling out a new monitor image, or when asked to canary, smoke-test or verify the monitor in production.
---

# Canary test in production

Run a one-task ECS service whose container reserves 32 MiB and fills 150 MiB, pinned to one host of an ECS cluster the monitor watches. The monitor on that host should stop it within a minute and go through the full stop flow (lock, `StopTask`, wait for exit, wait for the service to recover, `SHUTDOWN`). About 5 minutes end to end.

This changes production: tell the user before step 2. Everything is created with the AWS CLI, outside any infrastructure-as-code, and step 5 removes all of it.

## 0. Load the deployment's values

This repo is public, so the deployment's names live in `local.md` beside this file (gitignored): the AWS profile, cluster, capacity provider, lock table, and how to query the hosts' logs. Read it and set:

```bash
export AWS_PROFILE=<profile> AWS_REGION=<region>
CLUSTER=<cluster> CAPACITY_PROVIDER=<capacity provider> TABLE=<DYNAMODB_TABLE>
```

If `local.md` is missing, ask the user for these and for how to read the hosts' logs. Done when every value is set.

## 1. Pick a host

```bash
ARNS=$(aws ecs list-container-instances --cluster $CLUSTER --query containerInstanceArns --output text)
aws ecs describe-container-instances --cluster $CLUSTER --container-instances $ARNS \
  --query 'containerInstances[].{id:ec2InstanceId,tasks:runningTasksCount,memFree:remainingResources[?name==`MEMORY`].integerValue|[0]}' --output table
```

Choose an instance that already runs tasks and has a few hundred MiB free. An instance with 0 tasks may be about to scale in. Note its private DNS name (`aws ec2 describe-instances --instance-ids <id> --query 'Reservations[0].Instances[0].PrivateDnsName'`); host logs are usually keyed on it. Done when you have the instance ID and host name.

## 2. Start the canary

```bash
TD=$(aws ecs register-task-definition \
  --cli-input-json file://.claude/skills/canary-test/canary-task-definition.json \
  --query taskDefinition.taskDefinitionArn --output text)
aws ecs create-service --cluster $CLUSTER --service-name docker-monitor-canary \
  --task-definition "$TD" --desired-count 1 \
  --capacity-provider-strategy capacityProvider=$CAPACITY_PROVIDER,weight=1,base=0 \
  --placement-constraints 'type=memberOf,expression=ec2InstanceId == <instance-id>'
```

It has to be a **service**: after `StopTask` the monitor looks up the task's service and waits until desired equals running. A standalone `run-task` has no service, so that lookup fails and the monitor process exits.

## 3. Watch

The monitor's stdout (one JSON object per line, see `src/log.rs`) goes wherever the host sends the output of the process that runs it; `local.md` says where and how to query it. Watch the monitor's lines on that host, the canary's own output, and `aws ecs describe-services --cluster $CLUSTER --services docker-monitor-canary --query 'services[0].events'`, polling with a Monitor. Expected, in order:

1. `canary holding 150 MiB` (the canary's output)
2. `MEMORY_LIMIT_EXCEEDED` for `docker-monitor-canary`, a few seconds later
3. `Lock for stop_container created successfully`, then `refreshed successfully` every 15 s
4. An ECS service event starting a replacement task; the stopped task's `stoppedReason` is `Container over memory limit. Stopping to prevent OOM error.` and exit code 137 (Python as PID 1 ignores SIGTERM, so Docker kills it after the 30 s stop timeout)
5. `SHUTDOWN`, then `Lock for stop_container deleted successfully`, about a minute after step 2

A failure shows as `COULD_NOT_OBTAIN_LOCK`, an `Error:` line, or the monitor's process exiting. Done when `SHUTDOWN` or one of those appears.

## 4. Stop the repeat

The replacement task is also over its limit, and the monitor's next pass (60 s after `SHUTDOWN`) will catch it. Scale down right after the first `SHUTDOWN`:

```bash
aws ecs update-service --cluster $CLUSTER --service docker-monitor-canary --desired-count 0
aws ecs delete-service --cluster $CLUSTER --service docker-monitor-canary
```

The next pass can still catch the replacement before it stops: a second `MEMORY_LIMIT_EXCEEDED` → `SHUTDOWN` cycle is expected and harmless (the service is then 0 of 0, so the wait ends at once). Done when the last `MEMORY_LIMIT_EXCEEDED` has its matching `SHUTDOWN`.

## 5. Clean up and verify

```bash
aws ecs deregister-task-definition --task-definition docker-monitor-canary:<rev>
aws ecs delete-task-definitions --task-definitions docker-monitor-canary:<rev>
aws dynamodb scan --table-name $TABLE --query Count   # 0: no lock left behind
```

Done when the service is gone or `INACTIVE`, the task definition is deleted, the lock table is empty, and the monitor process hasn't exited since step 2. Report the timeline from step 3 to the user.
