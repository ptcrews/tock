#include <stdbool.h>
#include <stdlib.h>

#include "gpio.h"
#include "led.h"
#include "ieee802154.h"
#include "timer.h"
#include "tock.h"
#include "rng.h"
#include "alarm.h"

// IEEE 802.15.4 sample packet transmission app.
// Continually transmits frames at the specified short address to the specified
// destination address.

#define BUF_SIZE 60
char packet[BUF_SIZE];
bool toggle = true;

char packet_rx[IEEE802154_FRAME_LEN];

#define I_MIN 1000 // In ms
#define I_MAX 4    // Doublings of interval size
#define K 1        // Redundancy constant

typedef struct {
  int i;    // Current interval size
  int t;    // Time in current interval
  int c;    // Counter
  int val;  // Our current value
  tock_timer_t trickle_i_timer;
  tock_timer_t trickle_t_timer;
} trickle_state;

static int I_MAX_VAL = 0;
static trickle_state* global_state = NULL;


/*
static timer_t interval_timer;
static timer_t trickle_t_timer;
*/

void interval_t(trickle_state* state);
void interval_end(trickle_state* state);


static void t_timer_fired(__attribute__ ((unused)) int unused1,
                        __attribute__ ((unused)) int unused2,
                        __attribute__ ((unused)) int unused3,
                        __attribute__ ((unused)) void* arg) {
  printf("t fired\n");
  interval_t(((trickle_state*)arg));
}

static void interval_timer_fired(__attribute__ ((unused)) int unused1,
                        __attribute__ ((unused)) int unused2,
                        __attribute__ ((unused)) int unused3,
                        __attribute__ ((unused)) void* arg) {
  printf("i fired\n");
  interval_end(((trickle_state*)arg));
}


void initialize_state(trickle_state* state) {
  state->i = I_MIN;
  state->t = 0;
  state->c = 0;
  state->val = 0;

  I_MAX_VAL = I_MIN;
  int i;
  for (i = 0; i < I_MAX; i++) {
    I_MAX_VAL *= 2;
  }
  global_state = state;
}

void interval_start(trickle_state* state) {
  // Cancel all existing timers
  // TODO: This causes a crash if timers not set
  //timer_cancel(&state->trickle_i_timer);
  //timer_cancel(&state->trickle_t_timer);
  state->c = 0;
  int t = 0;
  // TODO: Figure out rng driver stuff
  int ret_val = rng_sync(((uint8_t*)(&t)), sizeof(int), sizeof(int));
  printf("t: %d\t retval: %d\n", t, ret_val);
  state->t = (t % (state->i/2)) + state->i/2;
  printf("t val: %d\t i val: %d\n", state->t, state->i);
  // Set a timer for time t ahead of us
  set_timer(state, state->t, false);
  set_timer(state, state->i, true);
}

void set_timer(trickle_state* state, int ms, bool set_interval_timer) {
  if (set_interval_timer) {
    timer_in(ms, interval_timer_fired, state, &state->trickle_i_timer);
  } else {
    timer_in(ms, t_timer_fired, state, &state->trickle_t_timer);
  }
}

void interval_t(trickle_state* state) {
  if (state->c < K) {
    transmit(state->val); 
  } 
}

// If the interval ended without hearing an inconsistent
// frame, we double our I val and restart the interval
void interval_end(trickle_state* state) {
  state->i = 2*state->i;
  printf("i: %d\t max_val: %d\n", state->i, I_MAX_VAL);
  if (state->i > I_MAX_VAL) {
    state->i = I_MAX_VAL;
  }
  interval_start(state);
}



static void receive_frame(__attribute__ ((unused)) int pans,
                          __attribute__ ((unused)) int dst_addr,
                          __attribute__ ((unused)) int src_addr,
                          __attribute__ ((unused)) void* ud) {
  printf("Packet received\n");
  // Re-subscribe to the callback, so that we again receive any frames
  ieee802154_receive(receive_frame, packet_rx, IEEE802154_FRAME_LEN);
  
  int offset = ieee802154_frame_get_payload_offset(packet_rx);
  int length = ieee802154_frame_get_payload_length(packet_rx);
  // TODO: Check PAN matches

  unsigned short short_addr;
  unsigned char long_addr[8];
  addr_mode_t addr_mode;
  addr_mode = ieee802154_frame_get_dst_addr(packet_rx, &short_addr, long_addr);
  if (addr_mode == ADDR_SHORT) {
    if (short_addr != 0xffff) {
      // TODO: Not for us(?)
      return;
    }
  } else if (addr_mode == ADDR_LONG) {
    int i;
    for (i = 0; i < 8; i++) {
      // TODO: Correct?
      if (long_addr[i] != 0xff) {
        return;
      }
    }
  } else {
    // Error: No address
    return;
  }

  // TODO: Don't really care about src addrs..

  if (length < sizeof(int)) {
    // Payload too short
    return;
  }
  int received_val = (int)packet_rx[offset];
  if (global_state->val == received_val) {
    consistent_transmission(global_state);
  } else {
    inconsistent_transmission(global_state, received_val);
  }
}

void consistent_transmission(trickle_state* state) {
  // Increment the "heard" counter
  state->c += 1;
}

void inconsistent_transmission(trickle_state* state, int val) {
  // Lets us detect when we need to update our value
  if (state->val < val) {
    state->val = val;
    // Toggle the gpio pin when we update our value - we use the
    // timing from this to measure propogation delay
    gpio_toggle(0);
    printf("New val: %d\n", val);
  }
  printf("Inconsistent transmission\n");
  if (state->i > I_MIN) {
    state->i = I_MIN;
    interval_start(state);
  }
}

void transmit(int payload) {
  *((int*)packet) = payload;
  int err = ieee802154_send(0xFFFF,         // Destination short MAC addr
                            SEC_LEVEL_NONE, // Security level
                            0,              // key_id_mode
                            NULL,           // key_id
                            packet,
                            sizeof(int));
  printf("Packet sent\n");
}

int main(void) {
  // Initialize radio, GPIO pin
  gpio_enable_output(0);
  ieee802154_set_address(0x1540);
  ieee802154_set_pan(0xABCD);
  ieee802154_config_commit();
  ieee802154_up();
  // This delay is necessary as if we receive a callback too early, we will
  // panic/crash
  delay_ms(1000);
  // Set our callback function as the callback
  ieee802154_receive(receive_frame, packet_rx, IEEE802154_FRAME_LEN);

  trickle_state* state = (trickle_state*)malloc(sizeof(trickle_state));
  initialize_state(state);
  interval_start(state);
}
