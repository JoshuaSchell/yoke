#!/usr/bin/env python3
"""A stand-in `claude -p --input-format stream-json` for Yoke's tests, speaking the lines the
real CLI (2.1.319) does, from what was seen of it (see the README's "Subagents and background
work"). One process takes message after message until its stdin closes, as a live CLI does.

What a message says decides what its turn does:
  "slow"        runs a 10 s foreground command (a task), which an interrupt stops and
                `background_tasks` moves to the background;
  "approve"     asks to run a Bash command and reports the answer ("allowed" / "denied", with
                "mode-bypassPermissions " once set_permission_mode switched to it);
  "stubborn"    takes 10 s and ignores an interrupt;
  "background"  starts a command in the background that finishes 1.5 s later, after which
                the CLI starts a turn of its own ("FINISHED");
  anything else replies "ok: <message> (<model>)".
A message with priority "now" ends the running turn at its next step and runs next. Each
message's uuid gets command_lifecycle lines. Every start appends the pid to `<this file>.runs`,
and every line read goes to `<this file>.in`.
"""
import json, os, queue, sys, threading, time

with open(sys.argv[0] + ".runs", "a") as f:
    f.write(f"{os.getpid()}\n")

model = "fake"
if "--model" in sys.argv:
    model = sys.argv[sys.argv.index("--model") + 1]
mode = sys.argv[sys.argv.index("--permission-mode") + 1] if "--permission-mode" in sys.argv else "default"
lock = threading.Lock()


def emit(obj):
    with lock:
        sys.stdout.write(json.dumps(obj) + "\n")
        sys.stdout.flush()


inbox = queue.Queue()


def read():
    for line in sys.stdin:
        line = line.strip()
        if line:
            with open(sys.argv[0] + ".in", "a") as f:
                f.write(line + "\n")
            inbox.put(json.loads(line))
    inbox.put(None)


threading.Thread(target=read, daemon=True).start()

pending = []        # messages waiting for a turn
answers = {}        # can_use_tool answers by request id
background = {}     # task id -> (ends at, tool call, description)
state = {"abort": False, "now": False, "move": False, "eof": False, "cost": 0.0, "n": 0}


def roster():
    emit({"type": "system", "subtype": "background_tasks_changed",
          "tasks": [{"task_id": t, "task_type": "local_bash", "description": d} for t, (_, _, d) in background.items()]})


def lifecycle(msg, st):
    if msg.get("uuid"):
        emit({"type": "command_lifecycle", "command_uuid": msg["uuid"], "state": st})


def handle(item):
    """One stdin line; returns False at EOF."""
    global model, mode
    if item is None:
        state["eof"] = True
        return False
    t = item.get("type")
    if t == "control_request":
        r, rid = item["request"], item["request_id"]
        sub = r.get("subtype")
        resp = {}
        if sub == "interrupt":
            state["abort"] = True
            if r.get("cancel_queued"):
                for m in pending:
                    lifecycle(m, "cancelled")
                pending.clear()
        elif sub == "background_tasks":
            state["move"] = True
        elif sub == "set_model":
            model = r.get("model") or "fake"
            resp = {"model": model}
        elif sub == "set_permission_mode":
            mode = r["mode"]
        elif sub == "stop_task":
            if r["task_id"] in background:
                _, call, d = background.pop(r["task_id"])
                emit({"type": "system", "subtype": "task_notification", "task_id": r["task_id"], "tool_use_id": call, "status": "stopped", "summary": d})
                roster()
        emit({"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": resp}})
    elif t == "control_response":
        answers[item["response"]["request_id"]] = item["response"].get("response", {})
    elif t == "user":
        if item.get("priority") == "now" and state.get("turn"):
            state["now"] = True
            pending.insert(0, item)
        else:
            pending.append(item)
        lifecycle(item, "queued")
    return True


def pump(timeout=0.02):
    try:
        handle(inbox.get(timeout=timeout))
    except queue.Empty:
        pass
    while True:
        try:
            handle(inbox.get_nowait())
        except queue.Empty:
            return


def text_of(msg):
    c = msg["message"]["content"]
    return c if isinstance(c, str) else " ".join(b.get("text", "") for b in c if b.get("type") == "text")


def result(subtype="success", reason="completed", error=False):
    state["cost"] += 0.001
    emit({"type": "result", "subtype": subtype, "is_error": error, "terminal_reason": reason, "session_id": "s1", "total_cost_usd": round(state["cost"], 4)})


def turn(msg, woke=False):
    state["turn"] = True
    state["abort"] = state["move"] = False
    # A message sent with priority "now" before this turn began ends it at its next step too.
    state["now"] = any(m.get("priority") == "now" for m in pending)
    emit({"type": "system", "subtype": "init", "session_id": "s1", "model": model, "capabilities": ["msg_lifecycle_v1"]})
    if not woke:
        lifecycle(msg, "started")
    text = "" if woke else text_of(msg)
    if woke:
        emit({"type": "assistant", "message": {"content": [{"type": "text", "text": "FINISHED"}]}, "parent_tool_use_id": None})
        result()
        return
    state["n"] += 1
    n = state["n"]
    if "slow" in text:
        call, task = f"slow{n}", f"bslow{n}"
        emit({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": call, "name": "Bash", "input": {"command": "sleep 10"}}]}, "parent_tool_use_id": None})
        emit({"type": "system", "subtype": "task_started", "task_id": task, "tool_use_id": call, "description": "Sleep 10", "is_backgrounded": False, "task_type": "local_bash"})
        ends = time.time() + 10
        while time.time() < ends:
            pump()
            if state["abort"]:
                emit({"type": "system", "subtype": "task_notification", "task_id": task, "tool_use_id": call, "status": "stopped", "summary": "Sleep 10"})
                emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": call, "content": "The user doesn't want to proceed with this tool use.", "is_error": True}]}, "parent_tool_use_id": None})
                result("error_during_execution", "aborted_tools", True)
                lifecycle(msg, "completed")
                return
            if state["move"]:
                background[task] = (ends, call, "Sleep 10")
                emit({"type": "system", "subtype": "task_updated", "task_id": task, "patch": {"is_backgrounded": True}})
                roster()
                emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": call, "content": "Command running in background with ID: " + task, "is_error": False}]}, "parent_tool_use_id": None})
                # A steer sends the message right behind its background_tasks.
                pump(0.2)
                break
        else:
            emit({"type": "system", "subtype": "task_notification", "task_id": task, "tool_use_id": call, "status": "completed", "summary": "Sleep 10"})
            emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": call, "content": "slow done", "is_error": False}]}, "parent_tool_use_id": None})
        if state["now"]:
            result("success", "aborted_tools")
            lifecycle(msg, "completed")
            return
    if "stubborn" in text:
        # Doesn't hear an interrupt: only ending it stops it.
        time.sleep(10)
    if "approve" in text:
        emit({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}}]}, "parent_tool_use_id": None})
        emit({"type": "control_request", "request_id": "r1", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "echo hi"}, "tool_use_id": "t1", "permission_suggestions": []}})
        while "r1" not in answers and not state["abort"] and not state["eof"]:
            pump()
        a = answers.pop("r1", {})
        out = "denied"
        if a.get("behavior") == "allow":
            out = ("mode-bypassPermissions " if mode == "bypassPermissions" else "") + "allowed"
        emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": out, "is_error": False}]}, "parent_tool_use_id": None})
    if "background" in text:
        call, task = f"bg{n}", f"bbg{n}"
        emit({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": call, "name": "Bash", "input": {"command": "sleep 1; echo BG_DONE", "run_in_background": True}}]}, "parent_tool_use_id": None})
        background[task] = (time.time() + 1.5, call, "Sleep then echo BG_DONE")
        roster()
        emit({"type": "system", "subtype": "task_started", "task_id": task, "tool_use_id": call, "description": "Sleep then echo BG_DONE", "is_backgrounded": True, "task_type": "local_bash"})
        emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": call, "content": "Command running in background with ID: " + task, "is_error": False}]}, "parent_tool_use_id": None})
    reply = "STARTED" if "background" in text else f"ok: {text} ({model})"
    emit({"type": "stream_event", "event": {"type": "message_start"}, "parent_tool_use_id": None})
    emit({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": reply}}, "parent_tool_use_id": None})
    emit({"type": "assistant", "message": {"content": [{"type": "text", "text": reply}]}, "parent_tool_use_id": None})
    result()
    lifecycle(msg, "completed")


while True:
    pump()
    now = time.time()
    woke = False
    for task, (ends, call, d) in list(background.items()):
        if now >= ends:
            del background[task]
            emit({"type": "system", "subtype": "task_updated", "task_id": task, "patch": {"status": "completed"}})
            emit({"type": "system", "subtype": "task_notification", "task_id": task, "tool_use_id": call, "status": "completed", "summary": d})
            roster()
            woke = True
    if woke:
        turn(None, woke=True)
        state["turn"] = False
        continue
    if pending:
        turn(pending.pop(0))
        state["turn"] = False
        continue
    if state["eof"] and not background:
        break
