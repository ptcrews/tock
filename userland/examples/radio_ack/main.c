#include <stdbool.h>
#include <stdio.h>

#include <led.h>
#include <radio.h>
#include <timer.h>

#define BUF_SIZE 60
char packet_rx[BUF_SIZE];
char packet_tx[BUF_SIZE];
bool toggle = true;

static void callback(__attribute__ ((unused)) int err,
                     __attribute__ ((unused)) int data_offset,
                     __attribute__ ((unused)) int data_len,
                     __attribute__ ((unused)) void* ud) {
  led_toggle(0);
  radio_receive_callback(callback, packet_rx, BUF_SIZE);
}

int main(void) {
  int i;
  char counter = 0;
  // printf("Starting 802.15.4 packet reception app.\n");
  for (i = 0; i < BUF_SIZE; i++) {
    packet_tx[i] = i;
    packet_rx[i] = 0;
  }
  radio_set_addr(0x802);
  radio_set_pan(0xABCD);
  radio_commit();
  radio_init();
  radio_receive_callback(callback, packet_rx, BUF_SIZE);
  while (1) {
    int err = radio_send(0x0802, packet_tx, BUF_SIZE);
    printf("Packet sent, return code: %i\n", err);
    counter++;
    packet_tx[0] = counter;
    delay_ms(4000);
  }
}
