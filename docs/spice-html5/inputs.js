"use strict";
/*
   Copyright (C) 2012 by Jeremy P. White <jwhite@codeweavers.com>

   This file is part of spice-html5.

   spice-html5 is free software: you can redistribute it and/or modify
   it under the terms of the GNU Lesser General Public License as published by
   the Free Software Foundation, either version 3 of the License, or
   (at your option) any later version.

   spice-html5 is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
   GNU Lesser General Public License for more details.

   You should have received a copy of the GNU Lesser General Public License
   along with spice-html5.  If not, see <http://www.gnu.org/licenses/>.
*/

import * as Messages from './spicemsg.js';
import { Constants } from './enums.js';
import { KeyNames } from './atKeynames.js';
import { SpiceConn } from './spiceconn.js';
import { DEBUG } from './utils.js';
import { code_to_scancode } from './code_to_scancode.js';

/*----------------------------------------------------------------------------
 ** Modifier Keystates
 **     These need to be tracked because focus in and out can get the keyboard
 **     out of sync.
 **------------------------------------------------------------------------*/
var Shift_state = -1;
var Ctrl_state = -1;
var Alt_state = -1;
var Meta_state = -1;

/* Extended (0xE0-prefixed) scancodes for the Meta/Super keys, matching
   the values in the scancode maps (utils.js / code_to_scancode.js). */
var META_L_SCAN = 0xE0 | (0x5B << 8);
var META_R_SCAN = 0xE0 | (0x5C << 8);

/*----------------------------------------------------------------------------
**  SpiceInputsConn
**      Drive the Spice Inputs channel (e.g. mouse + keyboard)
**--------------------------------------------------------------------------*/
function SpiceInputsConn()
{
    SpiceConn.apply(this, arguments);

    this.mousex = undefined;
    this.mousey = undefined;
    this.button_state = 0;
    this.waiting_for_ack = 0;

    /* Modifier tracking is module state; a page that tears down one
       connection and creates another must not inherit the previous
       session's held keys. */
    Shift_state = Ctrl_state = Alt_state = Meta_state = -1;
}

SpiceInputsConn.prototype = Object.create(SpiceConn.prototype);
SpiceInputsConn.prototype.process_channel_message = function(msg)
{
    if (msg.type == Constants.SPICE_MSG_INPUTS_INIT)
    {
        var inputs_init = new Messages.SpiceMsgInputsInit(msg.data);
        this.keyboard_modifiers = inputs_init.keyboard_modifiers;
        DEBUG > 1 && console.log("MsgInputsInit - modifier " + this.keyboard_modifiers);
        // FIXME - We don't do anything with the keyboard modifiers...
        return true;
    }
    if (msg.type == Constants.SPICE_MSG_INPUTS_KEY_MODIFIERS)
    {
        var key = new Messages.SpiceMsgInputsKeyModifiers(msg.data);
        this.keyboard_modifiers = key.keyboard_modifiers;
        DEBUG > 1 && console.log("MsgInputsKeyModifiers - modifier " + this.keyboard_modifiers);
        // FIXME - We don't do anything with the keyboard modifiers...
        return true;
    }
    if (msg.type == Constants.SPICE_MSG_INPUTS_MOUSE_MOTION_ACK)
    {
        DEBUG > 1 && console.log("mouse motion ack");
        this.waiting_for_ack -= Constants.SPICE_INPUT_MOTION_ACK_BUNCH;
        return true;
    }
    return false;
}



function handle_mousemove(e)
{
    /* Only build the message once we know it will be sent; mousemove can
       fire hundreds of times a second and the discarded-motion path was
       paying for two allocations and a serialize per event. */
    if (this.sc && this.sc.inputs && this.sc.inputs.state === "ready")
    {
        if (this.sc.inputs.waiting_for_ack < (2 * Constants.SPICE_INPUT_MOTION_ACK_BUNCH))
        {
            var msg = new Messages.SpiceMiniData();
            var move;
            if (this.sc.mouse_mode == Constants.SPICE_MOUSE_MODE_CLIENT)
            {
                move = new Messages.SpiceMsgcMousePosition(this.sc, e)
                msg.build_msg(Constants.SPICE_MSGC_INPUTS_MOUSE_POSITION, move);
            }
            else
            {
                move = new Messages.SpiceMsgcMouseMotion(this.sc, e)
                msg.build_msg(Constants.SPICE_MSGC_INPUTS_MOUSE_MOTION, move);
            }
            this.sc.inputs.send_msg(msg);
            this.sc.inputs.waiting_for_ack++;
        }
        else
        {
            DEBUG > 0 && this.sc.log_info("Discarding mouse motion");
        }
    }

    if (this.sc && this.sc.cursor && this.sc.cursor.spice_simulated_cursor)
    {
        /* The image sits inside the screen element, so it is placed in
           that element's own coordinates: the pointer's position within
           its box, undone for any CSS scale on it.  Page coordinates put
           it off by the screen's offset, and by the scale, on any page
           that does not draw the screen at the top-left corner. */
        var sim = this.sc.cursor.spice_simulated_cursor;
        var screen = sim.spice_screen;
        var rect = screen.getBoundingClientRect();
        var scale_x = screen.offsetWidth ? rect.width / screen.offsetWidth : 1;
        var scale_y = screen.offsetHeight ? rect.height / screen.offsetHeight : 1;
        sim.style.display = 'block';
        sim.style.left = (e.clientX - rect.left) / scale_x - sim.spice_hot_x + 'px';
        sim.style.top = (e.clientY - rect.top) / scale_y - sim.spice_hot_y + 'px';
        e.preventDefault();
    }

}

function handle_mousedown(e)
{
    var press = new Messages.SpiceMsgcMousePress(this.sc, e)
    var msg = new Messages.SpiceMiniData();
    msg.build_msg(Constants.SPICE_MSGC_INPUTS_MOUSE_PRESS, press);
    if (this.sc && this.sc.inputs && this.sc.inputs.state === "ready")
        this.sc.inputs.send_msg(msg);

    e.preventDefault();
}

function handle_contextmenu(e)
{
    e.preventDefault();
    return false;
}

function handle_mouseup(e)
{
    var release = new Messages.SpiceMsgcMouseRelease(this.sc, e)
    var msg = new Messages.SpiceMiniData();
    msg.build_msg(Constants.SPICE_MSGC_INPUTS_MOUSE_RELEASE, release);
    if (this.sc && this.sc.inputs && this.sc.inputs.state === "ready")
        this.sc.inputs.send_msg(msg);

    e.preventDefault();
}

function handle_mousewheel(e)
{
    var press = new Messages.SpiceMsgcMousePress;
    var release = new Messages.SpiceMsgcMouseRelease;
    if (e.deltaY < 0)
        press.button = release.button = Constants.SPICE_MOUSE_BUTTON_UP;
    else
        press.button = release.button = Constants.SPICE_MOUSE_BUTTON_DOWN;
    press.buttons_state = 0;
    release.buttons_state = 0;

    var msg = new Messages.SpiceMiniData();
    msg.build_msg(Constants.SPICE_MSGC_INPUTS_MOUSE_PRESS, press);
    if (this.sc && this.sc.inputs && this.sc.inputs.state === "ready")
        this.sc.inputs.send_msg(msg);

    msg.build_msg(Constants.SPICE_MSGC_INPUTS_MOUSE_RELEASE, release);
    if (this.sc && this.sc.inputs && this.sc.inputs.state === "ready")
        this.sc.inputs.send_msg(msg);

    e.preventDefault();
}

function handle_keydown(e)
{
    var key = new Messages.SpiceMsgcKeyDown(e)
    var msg = new Messages.SpiceMiniData();
    check_and_update_modifiers(e, key.code, this.sc);
    msg.build_msg(Constants.SPICE_MSGC_INPUTS_KEY_DOWN, key);
    if (this.sc && this.sc.inputs && this.sc.inputs.state === "ready")
        this.sc.inputs.send_msg(msg);

    e.preventDefault();
}

function handle_keyup(e)
{
    var key = new Messages.SpiceMsgcKeyUp(e)
    var msg = new Messages.SpiceMiniData();
    check_and_update_modifiers(e, key.code, this.sc);
    msg.build_msg(Constants.SPICE_MSGC_INPUTS_KEY_UP, key);
    if (this.sc && this.sc.inputs && this.sc.inputs.state === "ready")
        this.sc.inputs.send_msg(msg);

    e.preventDefault();
}

function sendCtrlAltDel(sc)
{
    if (sc && sc.inputs && sc.inputs.state === "ready"){
        var key = new Messages.SpiceMsgcKeyDown();
        var msg = new Messages.SpiceMiniData();

        update_modifier(true, KeyNames.KEY_LCtrl, sc);
        update_modifier(true, KeyNames.KEY_Alt, sc);

        key.code = KeyNames.KEY_KP_Decimal;
        msg.build_msg(Constants.SPICE_MSGC_INPUTS_KEY_DOWN, key);
        sc.inputs.send_msg(msg);
        /* The server forwards scancode bytes verbatim, so the release must
           carry the break bit itself, as keycode_to_end_scan does. */
        key.code = 0x80 | KeyNames.KEY_KP_Decimal;
        msg.build_msg(Constants.SPICE_MSGC_INPUTS_KEY_UP, key);
        sc.inputs.send_msg(msg);

        /* Release unless the key is positively known to be held: the state
           starts as the -1 sentinel (no key seen yet), and -1 == false is
           false, so testing against false left the synthetic Ctrl+Alt held
           in the guest whenever this ran before the first real keydown. */
        if(Ctrl_state !== true) update_modifier(false, KeyNames.KEY_LCtrl, sc);
        if(Alt_state !== true) update_modifier(false, KeyNames.KEY_Alt, sc);
    }
}

function update_modifier(state, code, sc)
{
    var msg = new Messages.SpiceMiniData();
    if (!state)
    {
        var key = new Messages.SpiceMsgcKeyUp()
        /* Extended (two-byte) scancodes carry the break bit in their second
           byte, as keycode_to_end_scan does; 0x80 would land in the 0xE0
           prefix and corrupt it. */
        key.code = code < 0x100 ? (0x80|code) : (0x8000|code);
        msg.build_msg(Constants.SPICE_MSGC_INPUTS_KEY_UP, key);
    }
    else
    {
        var key = new Messages.SpiceMsgcKeyDown()
        key.code = code;
        msg.build_msg(Constants.SPICE_MSGC_INPUTS_KEY_DOWN, key);
    }

    sc.inputs.send_msg(msg);
}

function check_and_update_modifiers(e, code, sc)
{
    if (Shift_state === -1)
    {
        Shift_state = e.shiftKey;
        Ctrl_state = e.ctrlKey;
        Alt_state = e.altKey;
        Meta_state = e.metaKey;
    }

    if (code === KeyNames.KEY_ShiftL)
        Shift_state = true;
    else if (code === KeyNames.KEY_Alt)
        Alt_state = true;
    else if (code === KeyNames.KEY_LCtrl)
        Ctrl_state = true;
    else if (code === META_L_SCAN || code === META_R_SCAN)
        Meta_state = true;
    else if (code === (0x80|KeyNames.KEY_ShiftL))
        Shift_state = false;
    else if (code === (0x80|KeyNames.KEY_Alt))
        Alt_state = false;
    else if (code === (0x80|KeyNames.KEY_LCtrl))
        Ctrl_state = false;
    else if (code === (0x8000|META_L_SCAN) || code === (0x8000|META_R_SCAN))
        Meta_state = false;

    if (sc && sc.inputs && sc.inputs.state === "ready")
    {
        if (Shift_state != e.shiftKey)
        {
            console.log("Shift state out of sync");
            update_modifier(e.shiftKey, KeyNames.KEY_ShiftL, sc);
            Shift_state = e.shiftKey;
        }
        if (Alt_state != e.altKey)
        {
            console.log("Alt state out of sync");
            update_modifier(e.altKey, KeyNames.KEY_Alt, sc);
            Alt_state = e.altKey;
        }
        if (Ctrl_state != e.ctrlKey)
        {
            console.log("Ctrl state out of sync");
            update_modifier(e.ctrlKey, KeyNames.KEY_LCtrl, sc);
            Ctrl_state = e.ctrlKey;
        }
        if (Meta_state != e.metaKey)
        {
            console.log("Meta state out of sync");
            update_modifier(e.metaKey, META_L_SCAN, sc);
            Meta_state = e.metaKey;
        }
    }
}

/*----------------------------------------------------------------------------
**  typeText
**      Deliver a string to the guest as synthetic keystrokes on the inputs
**  channel. The inputs channel speaks PC scancodes — positions on a physical
**  keyboard, not characters — so this maps each character to the key that
**  produces it on a US layout. A guest configured for another keymap will
**  type different characters, and anything a US layout cannot produce with
**  at most Shift (AltGr combinations, dead keys, non-ASCII) cannot be
**  expressed at all; such characters are skipped and reported back.
**--------------------------------------------------------------------------*/

/* character → [KeyboardEvent.code, needs-shift] on a US layout. */
var US_TYPEABLE = (function ()
{
    var map = {};
    var i;

    var letters = "abcdefghijklmnopqrstuvwxyz";
    for (i = 0; i < letters.length; i++)
    {
        var code = "Key" + letters[i].toUpperCase();
        map[letters[i]] = [code, false];
        map[letters[i].toUpperCase()] = [code, true];
    }

    var digits = "1234567890";
    var shifted_digits = "!@#$%^&*()";
    for (i = 0; i < digits.length; i++)
    {
        map[digits[i]] = ["Digit" + digits[i], false];
        map[shifted_digits[i]] = ["Digit" + digits[i], true];
    }

    var punctuation = [
        ["-", "_", "Minus"],
        ["=", "+", "Equal"],
        ["[", "{", "BracketLeft"],
        ["]", "}", "BracketRight"],
        ["\\", "|", "Backslash"],
        [";", ":", "Semicolon"],
        ["'", "\"", "Quote"],
        [",", "<", "Comma"],
        [".", ">", "Period"],
        ["/", "?", "Slash"],
        ["`", "~", "Backquote"],
    ];
    for (i = 0; i < punctuation.length; i++)
    {
        map[punctuation[i][0]] = [punctuation[i][2], false];
        map[punctuation[i][1]] = [punctuation[i][2], true];
    }

    map[" "] = ["Space", false];
    map["\n"] = ["Enter", false];
    map["\t"] = ["Tab", false];
    return map;
})();

function send_scancode(sc, scancode, down)
{
    var msg = new Messages.SpiceMiniData();
    var key;
    if (down)
    {
        key = new Messages.SpiceMsgcKeyDown();
        key.code = scancode;
        msg.build_msg(Constants.SPICE_MSGC_INPUTS_KEY_DOWN, key);
    }
    else
    {
        key = new Messages.SpiceMsgcKeyUp();
        /* Break code; every key this file types is a one-byte scancode. */
        key.code = 0x80 | scancode;
        msg.build_msg(Constants.SPICE_MSGC_INPUTS_KEY_UP, key);
    }
    sc.inputs.send_msg(msg);
}

/* A keystroke is a press and a release separated by time.  Sending both in
   the same tick, as this once did, makes a guest drop characters: Windows in
   particular discards a key whose break code arrives in the same instant as
   its make code, and the loss is silent and intermittent — a pasted URL or
   password arrives subtly wrong.  So hold each key down, and give Shift its
   own settle time either side, before moving on. */
var KEY_HOLD_MS = 12;
var SHIFT_SETTLE_MS = 12;

/* A server-side close leaves the channel's state at "ready", since the
   close handler only reports; the socket itself says whether keystrokes
   can still reach the guest. */
function inputs_live(sc)
{
    return sc && sc.inputs && sc.inputs.state === "ready" &&
           sc.inputs.ws && sc.inputs.ws.readyState === WebSocket.OPEN;
}

/* Resolves to { typed, skipped, aborted }: how many characters went out,
   which distinct characters had no US-layout key, and whether the inputs
   channel died mid-string. delay_ms is the gap between characters, which
   paces a large paste so a guest's keyboard buffer is not flooded. */
function typeText(sc, text, delay_ms)
{
    var delay = typeof delay_ms === 'number' ? delay_ms : 25;
    /* A CRLF must land as a single Enter, not Enter twice. */
    var chars = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
    var skipped = [];
    var typed = 0;
    var at = 0;

    return new Promise(function (resolve)
    {
        function step()
        {
            if (! inputs_live(sc))
            {
                resolve({ typed: typed, skipped: skipped, aborted: at < chars.length });
                return;
            }
            if (at >= chars.length)
            {
                resolve({ typed: typed, skipped: skipped, aborted: false });
                return;
            }

            var ch = chars[at++];
            var entry = US_TYPEABLE[ch];
            var scancode = entry && code_to_scancode[entry[0]];
            if (!scancode)
            {
                if (skipped.indexOf(ch) === -1)
                    skipped.push(ch);
                step();
                return;
            }

            /* Each phase is [what to send, how long to wait after it], run in
               order; the guest sees a press, a hold, and a release. */
            var shifted = entry[1];
            var phases = [];
            if (shifted)
                phases.push([function () { send_scancode(sc, KeyNames.KEY_ShiftL, true); }, SHIFT_SETTLE_MS]);
            phases.push([function () { send_scancode(sc, scancode, true); }, KEY_HOLD_MS]);
            phases.push([function () { send_scancode(sc, scancode, false); }, shifted ? SHIFT_SETTLE_MS : delay]);
            if (shifted)
                phases.push([function () { send_scancode(sc, KeyNames.KEY_ShiftL, false); }, delay]);
            typed++;

            var phase = 0;
            (function run()
            {
                if (phase >= phases.length)
                {
                    step();
                    return;
                }
                /* The channel can die mid-character; stop rather than send a
                   press whose release will never follow. */
                if (!(sc && sc.inputs && sc.inputs.state === "ready"))
                {
                    resolve({ typed: typed - 1, skipped: skipped, aborted: true });
                    return;
                }
                var current = phases[phase++];
                current[0]();
                window.setTimeout(run, current[1]);
            })();
        }
        step();
    });
}

export {
  SpiceInputsConn,
  handle_mousemove,
  handle_mousedown,
  handle_contextmenu,
  handle_mouseup,
  handle_mousewheel,
  handle_keydown,
  handle_keyup,
  sendCtrlAltDel,
  typeText,
};
