"""Tools a conversation model may call.

Each one runs on this Mac, reads at most a clock or the battery, and changes nothing. A tool
gives back the data the model is shown, a plain sentence that says the same (for when a
small model is handed the answer and still talks around it), the strings a faithful answer
would contain, and a few words to show the user.
"""

import ast
import datetime
import operator
import random
import re
import subprocess


def _time(args):
    now = datetime.datetime.now()
    clock = now.strftime("%I:%M %p").lstrip("0")
    date = f"{now:%A}, {now:%B} {now.day}, {now.year}"
    return {"time": clock, "date": date}, f"It's {clock} on {date}.", [clock, f"{now:%A}", f"{now:%B} {now.day}"], f"{clock}, {date}"


_WORDS = [
    (r"\bmultiplied by\b|\btimes\b|×|(?<=\d)\s*x\s*(?=\d)", "*"),
    (r"\bdivided by\b|\bover\b|÷", "/"),
    (r"\bplus\b", "+"),
    (r"\bminus\b", "-"),
    (r"\bto the power of\b|\^", "**"),
    (r"(\d+(?:\.\d+)?)\s*(?:%|percent)\s*of\b", r"(\1/100)*"),
    (r"(\d+(?:\.\d+)?)\s*(?:%|percent)", r"(\1/100)"),
    (r"(?<=\d),(?=\d{3}\b)", ""),
    (r"[=?]|\bwhat is\b|\bwhat's\b|\bcalculate\b", ""),
]
_OPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.Pow: operator.pow,
    ast.USub: operator.neg,
    ast.UAdd: operator.pos,
}


def _evaluate(node):
    """Arithmetic and nothing else: numbers and operators, no names and no calls."""
    if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)) and not isinstance(node.value, bool):
        return node.value
    if isinstance(node, ast.UnaryOp) and type(node.op) in _OPS:
        return _OPS[type(node.op)](_evaluate(node.operand))
    if isinstance(node, ast.BinOp) and type(node.op) in _OPS:
        left, right = _evaluate(node.left), _evaluate(node.right)
        if isinstance(node.op, ast.Pow) and (abs(right) > 64 or abs(left) > 1e6):
            raise ValueError("That power is too large.")
        return _OPS[type(node.op)](left, right)
    raise ValueError("Only arithmetic can be calculated.")


def _number(value):
    if isinstance(value, float) and value.is_integer() and abs(value) < 1e15:
        value = int(value)
    return f"{value:,}" if isinstance(value, int) else f"{value:,.6g}"


def _calculate(args):
    expression = str(args.get("expression") or next(iter(args.values()), "")).lower()
    for pattern, replacement in _WORDS:
        expression = re.sub(pattern, replacement, expression)
    expression = expression.strip()
    if not expression or len(expression) > 200:
        raise ValueError("There was nothing to calculate.")
    try:
        value = _evaluate(ast.parse(expression, mode="eval").body)
    except ZeroDivisionError:
        raise ValueError("That divides by zero.") from None
    except SyntaxError:
        raise ValueError(f"'{expression}' isn't arithmetic.") from None
    shown = _number(value)
    return {"expression": expression, "result": shown}, f"That comes to {shown}.", [shown, shown.replace(",", "")], shown


def _dice(args):
    sides = next((v for v in (args.get("sides"), *args.values()) if isinstance(v, int) and not isinstance(v, bool) and 2 <= v <= 1000), 6)
    rolled = random.randint(1, sides)
    return {"sides": sides, "rolled": rolled}, f"The die came up {rolled}.", [str(rolled)], f"{rolled} on a {sides}-sided die"


def _coin(args):
    side = random.choice(["heads", "tails"])
    return {"result": side}, f"It's {side}.", [side], side


def _battery(args):
    out = subprocess.run(["/usr/bin/pmset", "-g", "batt"], capture_output=True, text=True, timeout=5).stdout
    level = re.search(r"(\d+)%", out)
    if not level:
        return {"battery": "none", "power": "mains"}, "This Mac has no battery; it runs on mains power.", ["no battery", "mains", "plugged"], "no battery"
    charging = "AC Power" in out
    percent = f"{level.group(1)}%"
    return (
        {"battery": percent, "plugged_in": charging},
        f"The battery is at {level.group(1)} percent{', and plugged in' if charging else ''}.",
        [percent, f"{level.group(1)} percent"],
        percent + (", plugged in" if charging else ""),
    )


def _tool(name, description, run, **properties):
    return {"name": name, "description": description, "parameters": {"type": "object", "properties": properties, "required": list(properties)}, "run": run}


TOOLS = [
    _tool("get_current_time", "Get the current time, today's date and the day of the week.", _time),
    _tool(
        "calculate",
        "A calculator. Evaluate any arithmetic: add, subtract, multiply, divide, percentages.",
        _calculate,
        expression={"type": "string", "description": "The arithmetic to evaluate, e.g. 1234 * 56"},
    ),
    _tool(
        "roll_dice",
        "Roll one die with any number of sides and get the number rolled.",
        _dice,
        sides={"type": "integer", "description": "How many sides the die has, 6 by default"},
    ),
    _tool("flip_coin", "Flip a coin and get heads or tails.", _coin),
    _tool("get_battery_level", "Get the battery charge of this computer.", _battery),
]
BY_NAME = {tool["name"]: tool for tool in TOOLS}


def schema(tool):
    """What a model is told about a tool."""
    return {key: tool[key] for key in ("name", "description", "parameters")}


def parse_arguments(text):
    """The keyword arguments of a call written the way Python writes one: `sides=20, x="y"`.
    Small models write these loosely, so anything that doesn't parse is dropped."""
    try:
        call = ast.parse(f"f({text})", mode="eval").body
    except SyntaxError:
        return {}
    args = {}
    for keyword in call.keywords:
        try:
            args[keyword.arg] = ast.literal_eval(keyword.value)
        except (ValueError, SyntaxError):
            pass
    for index, value in enumerate(call.args):
        try:
            args[f"_{index}"] = ast.literal_eval(value)
        except (ValueError, SyntaxError):
            pass
    return {key: value for key, value in args.items() if key and isinstance(value, (str, int, float)) and not isinstance(value, bool)}


def call(name, args):
    """Runs a tool. Returns (what the model is shown, a sentence saying it, strings one of
    which a faithful answer contains, the result in a few words)."""
    return BY_NAME[name]["run"](args)


def written(name, args):
    """A call as it is shown to the model and to the user: `roll_dice(sides=20)`."""
    known = BY_NAME[name]["parameters"]["properties"]
    inside = ", ".join(f"{key}={value!r}" if not isinstance(value, str) else f'{key}="{value}"' for key, value in args.items() if key in known)
    return f"{name}({inside})"


_ONES = "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen".split()
_TENS = "twenty thirty forty fifty sixty seventy eighty ninety".split()


def _in_words(n):
    if n < 20:
        return _ONES[n]
    if n < 100:
        return _TENS[n // 10 - 2] + ("-" + _ONES[n % 10] if n % 10 else "")
    if n < 1000:
        return _ONES[n // 100] + " hundred" + (" " + _in_words(n % 100) if n % 100 else "")
    for size, name in ((10**12, "trillion"), (10**9, "billion"), (10**6, "million"), (10**3, "thousand")):
        if n >= size:
            return _in_words(n // size) + " " + name + (" " + _in_words(n % size) if n % size else "")


def say_numbers(text):
    """Writes whole numbers of four digits and more out in words, for a voice that stumbles
    over long runs of digits: 69,104 becomes sixty-nine thousand one hundred four."""

    def words(match):
        digits = match.group(0).replace(",", "")
        return _in_words(int(digits)) if len(digits) <= 15 else match.group(0)

    return re.sub(r"(?<![\d.:])(?:\d{1,3}(?:,\d{3})+|\d{4,})(?![\d:]|\.\d)", words, text)
