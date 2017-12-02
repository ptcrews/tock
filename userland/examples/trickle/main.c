#include <stdbool.h>

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
#define I_MAX 10   // Doublings of interval size
#define K 5        // Redundancy constant

int i_cur = I_MIN; // Current interval size
int t = 0; // Time in current interval
int c = 0; // Counter

static timer_t interval_timer;
static timer_t t_timer;

int val = 0;

void initialize_state(void) {
  i_cur = I_MIN;
}

void interval_start(void) {
  // Cancel all existing timers
  timer_cancel(&interval_timer);
  timer_cancel(&t_timer);
  c = 0;
  //t = i_cur / 2 + 1; // Random point in interval [i_cur/2, i_cur)
  int t = 0;
  int ret_val = rng_sync(((uint8_t*)(&t)), sizeof(int), sizeof(int));
  t = (t % (i_cur/2)) + i_cur/2;
  // Set a timer for time t ahead of us
  set_timer(t, false);
  set_timer(i_cur, true);
}

static void interval_t(void);
static void interval_end(void);

void set_timer(int ms, bool set_interval_timer) {
  if (set_interval_timer) {
    timer_in(ms, interval_end, NULL, &interval_timer);
  } else {
    timer_in(ms, interval_t, NULL, &t_timer);
  }
}

void interval_t(void) {
  if (c < K) {
    transmit(val); 
  } 
}

void interval_end(void) {
  i_cur = 2*i_cur;
  if (i_cur > I_MAX) {
    i_cur = I_MAX;
  }
  interval_start();
}


static void receive_frame(__attribute__ ((unused)) int pans,
                          __attribute__ ((unused)) int dst_addr,
                          __attribute__ ((unused)) int src_addr,
                          __attribute__ ((unused)) void* ud) {
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
  if (val == (int)packet_rx[offset]) {
    consistent_transmission();
  } else {
    inconsistent_transmission();
  }
}

void consistent_transmission(void) {
  c += 1;
}

void inconsistent_transmission(void) {
  if (i_cur > I_MIN) {
    i_cur = I_MIN;
    interval_start();
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
}

int main(void) {
  // Initialize radio, GPIO pin
  gpio_enable_output(0);
  ieee802154_set_address(0x1540);
  ieee802154_set_pan(0xABCD);
  ieee802154_config_commit();
  ieee802154_up();
  // Set our callback function as the callback
  ieee802154_receive(receive_frame, packet_rx, IEEE802154_FRAME_LEN);
  /*
    led_toggle(0);
    if (err != TOCK_SUCCESS) {
      gpio_toggle(0);
    } else {
      printf("Success\n");
    }
    delay_ms(250);
  }
    */
}
