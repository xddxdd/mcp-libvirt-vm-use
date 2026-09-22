/* -*- Mode: C; c-basic-offset: 4; indent-tabs-mode: nil -*- */
/*
   Copyright (C) 2009 Red Hat, Inc.

   This library is free software; you can redistribute it and/or
   modify it under the terms of the GNU Lesser General Public
   License as published by the Free Software Foundation; either
   version 2.1 of the License, or (at your option) any later version.

   This library is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
   Lesser General Public License for more details.

   You should have received a copy of the GNU Lesser General Public
   License along with this library; if not, see <http://www.gnu.org/licenses/>.
*/
#include <config.h>

#include <spice/macros.h>
#include <spice/protocol.h>
#include <spice/vd_agent.h>

#include <common/marshaller.h>
#include <common/messages.h>
#include <common/generated_server_marshallers.h>
#include <common/demarshallers.h>

#include "spice-wrapped.h"
#include "red-common.h"
#include "reds.h"
#include "red-stream.h"
#include "red-channel.h"
#include "red-channel-client.h"
#include "red-client.h"
#include "inputs-channel-client.h"
#include "main-channel-client.h"
#include "inputs-channel.h"
#include "migration-protocol.h"
#include "utils.h"

struct SpiceKbdState {
    uint8_t push_ext_type;

    /* track key press state */
    bool key[0x80];
    bool key_ext[0x80];
    InputsChannel *inputs;
};

static SpiceKbdState* spice_kbd_state_new(InputsChannel *inputs)
{
    auto st = g_new0(SpiceKbdState, 1);
    st->inputs = inputs;
    return st;
}

struct SpiceMouseState {
    RedsState *reds;
};

static SpiceMouseState* spice_mouse_state_new(RedsState *reds)
{
    auto st = g_new0(SpiceMouseState, 1);
    st->reds = reds;
    return st;
}

struct SpiceTabletState {
    RedsState *reds;
};

static SpiceTabletState* spice_tablet_state_new(RedsState* reds)
{
    auto st = g_new0(SpiceTabletState, 1);
    st->reds = reds;
    return st;
}

static void spice_tablet_state_free(SpiceTabletState* st)
{
    g_free(st);
}

RedsState* spice_tablet_state_get_server(SpiceTabletState *st)
{
    return st->reds;
}

RedsState* spice_kbd_state_get_server(SpiceKbdState *st)
{
    return st->inputs->get_server();
}

RedsState* spice_mouse_state_get_server(SpiceMouseState *st)
{
    return st->reds;
}

struct RedKeyModifiersPipeItem: public RedPipeItemNum<RED_PIPE_ITEM_KEY_MODIFIERS> {
    explicit RedKeyModifiersPipeItem(uint8_t modifiers);
    uint8_t modifiers;
};

struct RedInputsInitPipeItem: public RedPipeItemNum<RED_PIPE_ITEM_INPUTS_INIT> {
    explicit RedInputsInitPipeItem(uint8_t modifiers);
    uint8_t modifiers;
};


#define KEY_MODIFIERS_TTL (MSEC_PER_SEC * 2)

#define SCAN_CODE_RELEASE 0x80
#define SCROLL_LOCK_SCAN_CODE 0x46
#define NUM_LOCK_SCAN_CODE 0x45
#define CAPS_LOCK_SCAN_CODE 0x3a

void InputsChannel::set_tablet_logical_size(int x_res, int y_res)
{
    SpiceTabletInterface *sif;

    sif = SPICE_UPCAST(SpiceTabletInterface, tablet->base.sif);
    sif->set_logical_size(tablet, x_res, y_res);
}

const VDAgentMouseState *InputsChannel::get_mouse_state()
{
    return &mouse_state;
}

// middle and right states are inverted
// all buttons from SPICE_MOUSE_BUTTON_MASK_SIDE are mapped a bit higher
// to avoid conflicting with some internal Qemu bit
#define RED_MOUSE_STATE_TO_LOCAL(state)                           \
    ((state & SPICE_MOUSE_BUTTON_MASK_LEFT) |                     \
     ((state & (SPICE_MOUSE_BUTTON_MASK_MIDDLE|0xffe0)) << 1) |   \
     ((state & SPICE_MOUSE_BUTTON_MASK_RIGHT) >> 1))

// mouse button constants are defined to be off-one between agent and SPICE protocol
#define RED_MOUSE_BUTTON_STATE_TO_AGENT(state) ((state) << 1)

void InputsChannel::activate_modifiers_watch()
{
    red_timer_start(key_modifiers_timer, KEY_MODIFIERS_TTL);
}

static void kbd_push_scan(SpiceKbdInstance *sin, uint8_t scan)
{
    SpiceKbdInterface *sif;

    if (!sin) {
        return;
    }
    sif = SPICE_UPCAST(SpiceKbdInterface, sin->base.sif);

    /* track XT scan code set 1 key state */
    if (scan >= 0xe0 && scan <= 0xe2) {
        sin->st->push_ext_type = scan;
    } else {
        if (sin->st->push_ext_type == 0 || sin->st->push_ext_type == 0xe0) {
            bool *state = sin->st->push_ext_type ? sin->st->key_ext : sin->st->key;
            state[scan & 0x7f] = !(scan & SCAN_CODE_RELEASE);
        }
        sin->st->push_ext_type = 0;
    }

    sif->push_scan_freg(sin, scan);
}

static uint8_t scancode_to_modifier_flag(uint8_t scancode)
{
    switch (scancode & ~SCAN_CODE_RELEASE) {
    case CAPS_LOCK_SCAN_CODE:
        return SPICE_KEYBOARD_MODIFIER_FLAGS_CAPS_LOCK;
    case NUM_LOCK_SCAN_CODE:
        return SPICE_KEYBOARD_MODIFIER_FLAGS_NUM_LOCK;
    case SCROLL_LOCK_SCAN_CODE:
        return SPICE_KEYBOARD_MODIFIER_FLAGS_SCROLL_LOCK;
    }
    return 0;
}

void InputsChannel::sync_locks(uint8_t scan)
{
    uint8_t change_modifier = scancode_to_modifier_flag(scan);

    if (scan & SCAN_CODE_RELEASE) { /* KEY_UP */
        modifiers_pressed &= ~change_modifier;
    } else {  /* KEY_DOWN */
        if (change_modifier && !(modifiers_pressed & change_modifier)) {
            modifiers ^= change_modifier;
            modifiers_pressed |= change_modifier;
            activate_modifiers_watch();
        }
    }
}

static uint8_t kbd_get_leds(SpiceKbdInstance *sin)
{
    SpiceKbdInterface *sif;

    if (!sin) {
        return 0;
    }
    sif = SPICE_UPCAST(SpiceKbdInterface, sin->base.sif);
    return sif->get_leds(sin);
}

RedKeyModifiersPipeItem::RedKeyModifiersPipeItem(uint8_t init_modifiers):
    modifiers(init_modifiers)
{
}

void InputsChannelClient::send_item(RedPipeItem *base)
{
    SpiceMarshaller *m = get_marshaller();

    switch (base->type) {
        case RED_PIPE_ITEM_KEY_MODIFIERS:
        {
            SpiceMsgInputsKeyModifiers key_modifiers;

            init_send_data(SPICE_MSG_INPUTS_KEY_MODIFIERS);
            key_modifiers.modifiers =
                static_cast<RedKeyModifiersPipeItem*>(base)->modifiers;
            spice_marshall_msg_inputs_key_modifiers(m, &key_modifiers);
            break;
        }
        case RED_PIPE_ITEM_INPUTS_INIT:
        {
            SpiceMsgInputsInit inputs_init;

            init_send_data(SPICE_MSG_INPUTS_INIT);
            inputs_init.keyboard_modifiers =
                static_cast<RedInputsInitPipeItem*>(base)->modifiers;
            spice_marshall_msg_inputs_init(m, &inputs_init);
            break;
        }
        case RED_PIPE_ITEM_MOUSE_MOTION_ACK:
            init_send_data(SPICE_MSG_INPUTS_MOUSE_MOTION_ACK);
            break;
        case RED_PIPE_ITEM_MIGRATE_DATA:
            get_channel()->src_during_migrate = FALSE;
            send_migrate_data(m, base);
            break;
        default:
            spice_warning("invalid pipe iten %d", base->type);
            break;
    }
    begin_send_message();
}

bool InputsChannelClient::handle_message(uint16_t type, uint32_t size, void *message)
{
    InputsChannel *inputs_channel = get_channel();
    uint32_t i;
    RedsState *reds = inputs_channel->get_server();

    switch (type) {
    case SPICE_MSGC_INPUTS_KEY_DOWN: {
        auto key_down = static_cast<SpiceMsgcKeyDown *>(message);
        inputs_channel->sync_locks(key_down->code);
    }
        /* fallthrough */
    case SPICE_MSGC_INPUTS_KEY_UP: {
        auto key_up = static_cast<SpiceMsgcKeyUp *>(message);
        for (i = 0; i < 4; i++) {
            uint8_t code = (key_up->code >> (i * 8)) & 0xff;
            if (code == 0) {
                break;
            }
            kbd_push_scan(inputs_channel->keyboard, code);
            inputs_channel->sync_locks(code);
        }
        break;
    }
    case SPICE_MSGC_INPUTS_KEY_SCANCODE: {
        auto code = static_cast<uint8_t *>(message);
        for (i = 0; i < size; i++) {
            kbd_push_scan(inputs_channel->keyboard, code[i]);
            inputs_channel->sync_locks(code[i]);
        }
        break;
    }
    case SPICE_MSGC_INPUTS_MOUSE_MOTION: {
        SpiceMouseInstance *mouse = inputs_channel->mouse;
        auto mouse_motion = static_cast<SpiceMsgcMouseMotion *>(message);

        on_mouse_motion();
        if (mouse && reds_get_mouse_mode(reds) == SPICE_MOUSE_MODE_SERVER) {
            SpiceMouseInterface *sif;
            sif = SPICE_UPCAST(SpiceMouseInterface, mouse->base.sif);
            sif->motion(mouse,
                        mouse_motion->dx, mouse_motion->dy, 0,
                        RED_MOUSE_STATE_TO_LOCAL(mouse_motion->buttons_state));
        }
        break;
    }
    case SPICE_MSGC_INPUTS_MOUSE_POSITION: {
        auto pos = static_cast<SpiceMsgcMousePosition *>(message);
        SpiceTabletInstance *tablet = inputs_channel->tablet;

        on_mouse_motion();
        if (reds_get_mouse_mode(reds) != SPICE_MOUSE_MODE_CLIENT) {
            break;
        }
        spice_assert((reds_config_get_agent_mouse(reds) && reds_has_vdagent(reds)) || tablet);
        if (!reds_config_get_agent_mouse(reds) || !reds_has_vdagent(reds)) {
            SpiceTabletInterface *sif;
            sif = SPICE_UPCAST(SpiceTabletInterface, tablet->base.sif);
            sif->position(tablet, pos->x, pos->y, RED_MOUSE_STATE_TO_LOCAL(pos->buttons_state));
            break;
        }
        VDAgentMouseState *mouse_state = &inputs_channel->mouse_state;
        mouse_state->x = pos->x;
        mouse_state->y = pos->y;
        mouse_state->buttons = RED_MOUSE_BUTTON_STATE_TO_AGENT(pos->buttons_state);
        mouse_state->display_id = pos->display_id;
        reds_handle_agent_mouse_event(reds, mouse_state);
        break;
    }
    case SPICE_MSGC_INPUTS_MOUSE_PRESS: {
        auto mouse_press = static_cast<SpiceMsgcMousePress *>(message);
        int dz = 0;
        if (mouse_press->button == SPICE_MOUSE_BUTTON_UP) {
            dz = -1;
        } else if (mouse_press->button == SPICE_MOUSE_BUTTON_DOWN) {
            dz = 1;
        }
        if (reds_get_mouse_mode(reds) == SPICE_MOUSE_MODE_CLIENT) {
            if (reds_config_get_agent_mouse(reds) && reds_has_vdagent(reds)) {
                inputs_channel->mouse_state.buttons =
                    RED_MOUSE_BUTTON_STATE_TO_AGENT(mouse_press->buttons_state) |
                    (dz == -1 ? VD_AGENT_UBUTTON_MASK : 0) |
                    (dz == 1 ? VD_AGENT_DBUTTON_MASK : 0);
                reds_handle_agent_mouse_event(reds, &inputs_channel->mouse_state);
            } else if (inputs_channel->tablet) {
                SpiceTabletInterface *sif;
                sif = SPICE_CONTAINEROF(inputs_channel->tablet->base.sif,
                                        SpiceTabletInterface, base);
                sif->wheel(inputs_channel->tablet, dz,
                           RED_MOUSE_STATE_TO_LOCAL(mouse_press->buttons_state));
            }
        } else if (inputs_channel->mouse) {
            SpiceMouseInterface *sif;
            sif = SPICE_CONTAINEROF(inputs_channel->mouse->base.sif,
                                    SpiceMouseInterface, base);
            sif->motion(inputs_channel->mouse, 0, 0, dz,
                        RED_MOUSE_STATE_TO_LOCAL(mouse_press->buttons_state));
        }
        break;
    }
    case SPICE_MSGC_INPUTS_MOUSE_RELEASE: {
        auto mouse_release = static_cast<SpiceMsgcMouseRelease *>(message);
        if (reds_get_mouse_mode(reds) == SPICE_MOUSE_MODE_CLIENT) {
            if (reds_config_get_agent_mouse(reds) && reds_has_vdagent(reds)) {
                inputs_channel->mouse_state.buttons =
                    RED_MOUSE_BUTTON_STATE_TO_AGENT(mouse_release->buttons_state);
                reds_handle_agent_mouse_event(reds, &inputs_channel->mouse_state);
            } else if (inputs_channel->tablet) {
                SpiceTabletInterface *sif;
                sif = SPICE_CONTAINEROF(inputs_channel->tablet->base.sif,
                                        SpiceTabletInterface, base);
                sif->buttons(inputs_channel->tablet,
                             RED_MOUSE_STATE_TO_LOCAL(mouse_release->buttons_state));
            }
        } else if (inputs_channel->mouse) {
            SpiceMouseInterface *sif;
            sif = SPICE_CONTAINEROF(inputs_channel->mouse->base.sif,
                                    SpiceMouseInterface, base);
            sif->buttons(inputs_channel->mouse,
                         RED_MOUSE_STATE_TO_LOCAL(mouse_release->buttons_state));
        }
        break;
    }
    case SPICE_MSGC_INPUTS_KEY_MODIFIERS: {
        auto modifiers = static_cast<SpiceMsgcKeyModifiers *>(message);
        uint8_t leds;
        SpiceKbdInstance *keyboard = inputs_channel->keyboard;

        if (!keyboard) {
            break;
        }
        leds = inputs_channel->modifiers;
        if (!(inputs_channel->modifiers_pressed & SPICE_KEYBOARD_MODIFIER_FLAGS_SCROLL_LOCK) &&
            ((modifiers->modifiers & SPICE_KEYBOARD_MODIFIER_FLAGS_SCROLL_LOCK) !=
             (leds & SPICE_KEYBOARD_MODIFIER_FLAGS_SCROLL_LOCK))) {
            kbd_push_scan(keyboard, SCROLL_LOCK_SCAN_CODE);
            kbd_push_scan(keyboard, SCROLL_LOCK_SCAN_CODE | SCAN_CODE_RELEASE);
            inputs_channel->modifiers ^= SPICE_KEYBOARD_MODIFIER_FLAGS_SCROLL_LOCK;
        }
        if (!(inputs_channel->modifiers_pressed & SPICE_KEYBOARD_MODIFIER_FLAGS_NUM_LOCK) &&
            ((modifiers->modifiers & SPICE_KEYBOARD_MODIFIER_FLAGS_NUM_LOCK) !=
             (leds & SPICE_KEYBOARD_MODIFIER_FLAGS_NUM_LOCK))) {
            kbd_push_scan(keyboard, NUM_LOCK_SCAN_CODE);
            kbd_push_scan(keyboard, NUM_LOCK_SCAN_CODE | SCAN_CODE_RELEASE);
            inputs_channel->modifiers ^= SPICE_KEYBOARD_MODIFIER_FLAGS_NUM_LOCK;
        }
        if (!(inputs_channel->modifiers_pressed & SPICE_KEYBOARD_MODIFIER_FLAGS_CAPS_LOCK) &&
            ((modifiers->modifiers & SPICE_KEYBOARD_MODIFIER_FLAGS_CAPS_LOCK) !=
             (leds & SPICE_KEYBOARD_MODIFIER_FLAGS_CAPS_LOCK))) {
            kbd_push_scan(keyboard, CAPS_LOCK_SCAN_CODE);
            kbd_push_scan(keyboard, CAPS_LOCK_SCAN_CODE | SCAN_CODE_RELEASE);
            inputs_channel->modifiers ^= SPICE_KEYBOARD_MODIFIER_FLAGS_CAPS_LOCK;
        }
        inputs_channel->activate_modifiers_watch();
        break;
    }
    default:
        return RedChannelClient::handle_message(type, size, message);
    }
    return TRUE;
}

void InputsChannel::release_keys()
{
    int i;
    SpiceKbdState *st;

    if (!keyboard) {
        return;
    }
    st = keyboard->st;

    for (i = 0; i < SPICE_N_ELEMENTS(st->key); i++) {
        if (!st->key[i])
            continue;

        st->key[i] = FALSE;
        kbd_push_scan(keyboard, i | SCAN_CODE_RELEASE);
    }

    for (i = 0; i < SPICE_N_ELEMENTS(st->key_ext); i++) {
        if (!st->key_ext[i])
            continue;

        st->key_ext[i] = FALSE;
        kbd_push_scan(keyboard, 0xe0);
        kbd_push_scan(keyboard, i | SCAN_CODE_RELEASE);
    }
}

RedInputsInitPipeItem::RedInputsInitPipeItem(uint8_t init_modifiers):
    modifiers(init_modifiers)
{
}

void InputsChannelClient::pipe_add_init()
{
    auto modifiers = kbd_get_leds(get_channel()->keyboard);
    pipe_add_push(red::make_shared<RedInputsInitPipeItem>(modifiers));
}

void InputsChannel::on_connect(RedClient *client, RedStream *stream, int migration,
                               RedChannelCapabilities *caps)
{
    if (!red_stream_is_ssl(stream) && !client->during_migrate_at_target()) {
        client->get_main()->push_notify("keyboard channel is insecure");
    }

    inputs_channel_client_create(this, client, stream, caps);
}

void InputsChannelClient::migrate()
{
    InputsChannel *inputs = get_channel();
    inputs->src_during_migrate = true;
    RedChannelClient::migrate();
}

void InputsChannel::push_keyboard_modifiers()
{
    if (!is_connected() || src_during_migrate) {
        return;
    }
    pipes_add(red::make_shared<RedKeyModifiersPipeItem>(modifiers));
}

SPICE_GNUC_VISIBLE int spice_server_kbd_leds(SpiceKbdInstance *sin, int leds)
{
    InputsChannel *inputs_channel = sin->st->inputs;
    if (inputs_channel) {
        inputs_channel->modifiers = leds;
        inputs_channel->push_keyboard_modifiers();
    }
    return 0;
}

void InputsChannel::key_modifiers_sender(InputsChannel *inputs)
{
    inputs->push_keyboard_modifiers();
}

void InputsChannelClient::handle_migrate_flush_mark()
{
    pipe_add_type(RED_PIPE_ITEM_MIGRATE_DATA);
}

bool InputsChannelClient::handle_migrate_data(uint32_t size, void *message)
{
    InputsChannel *inputs = get_channel();
    SpiceMigrateDataHeader *header;
    SpiceMigrateDataInputs *mig_data;

    if (size < sizeof(SpiceMigrateDataHeader) + sizeof(SpiceMigrateDataInputs)) {
        spice_warning("bad message size %u", size);
        return FALSE;
    }

    header = static_cast<SpiceMigrateDataHeader *>(message);
    mig_data = reinterpret_cast<SpiceMigrateDataInputs *>(header + 1);

    if (!migration_protocol_validate_header(header,
                                            SPICE_MIGRATE_DATA_INPUTS_MAGIC,
                                            SPICE_MIGRATE_DATA_INPUTS_VERSION)) {
        spice_error("bad header");
        return FALSE;
    }
    InputsChannel::key_modifiers_sender(inputs);
    handle_migrate_data(mig_data->motion_count);
    return TRUE;
}

red::shared_ptr<InputsChannel> inputs_channel_new(RedsState *reds)
{
    return red::make_shared<InputsChannel>(reds);
}

InputsChannel::InputsChannel(RedsState *reds):
    RedChannel(reds, SPICE_CHANNEL_INPUTS, 0, RedChannel::MigrateAll)
{
    SpiceCoreInterfaceInternal *core = get_core_interface();

    set_cap(SPICE_INPUTS_CAP_KEY_SCANCODE);
    reds_register_channel(reds, this);

    key_modifiers_timer = core->timer_new(key_modifiers_sender, this);
    if (!key_modifiers_timer) {
        spice_error("key modifiers timer create failed");
    }
}

InputsChannel::~InputsChannel()
{
    detach_keyboard(keyboard);
    detach_mouse(mouse);
    detach_tablet(tablet);
    red_timer_remove(key_modifiers_timer);
}

int InputsChannel::set_keyboard(SpiceKbdInstance *new_keyboard)
{
    if (keyboard) {
        red_channel_warning(this, "already have keyboard");
        return -1;
    }
    keyboard = new_keyboard;
    keyboard->st = spice_kbd_state_new(this);
    return 0;
}

int InputsChannel::set_mouse(SpiceMouseInstance *new_mouse)
{
    if (mouse) {
        red_channel_warning(this, "already have mouse");
        return -1;
    }
    mouse = new_mouse;
    mouse->st = spice_mouse_state_new(get_server());
    return 0;
}

int InputsChannel::set_tablet(SpiceTabletInstance *new_tablet)
{
    if (tablet) {
        red_channel_warning(this, "already have tablet");
        return -1;
    }
    tablet = new_tablet;
    tablet->st = spice_tablet_state_new(get_server());
    return 0;
}

bool InputsChannel::has_tablet() const
{
    return tablet != nullptr;
}

void InputsChannel::detach_tablet(SpiceTabletInstance *old_tablet)
{
    if (old_tablet != nullptr && old_tablet == tablet) {
        spice_tablet_state_free(old_tablet->st);
        old_tablet->st = nullptr;
    }
    tablet = nullptr;
}

void InputsChannel::detach_keyboard(SpiceKbdInstance *old_keyboard)
{
    if (old_keyboard != nullptr && old_keyboard == keyboard) {
        g_free(old_keyboard->st);
        old_keyboard->st = nullptr;
    }
    keyboard = nullptr;
}

void InputsChannel::detach_mouse(SpiceMouseInstance *old_mouse)
{
    if (old_mouse != nullptr && old_mouse == mouse) {
        g_free(old_mouse->st);
        old_mouse->st = nullptr;
    }
    mouse = nullptr;
}

bool InputsChannel::is_src_during_migrate() const
{
    return src_during_migrate;
}
