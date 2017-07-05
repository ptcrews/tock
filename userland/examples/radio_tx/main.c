#include <stdbool.h>
#include <stdio.h>

#include "gpio.h"
#include "led.h"
#include "radio.h"
#include "timer.h"
#include "tock.h"

#define BUF_SIZE 60
char packet[BUF_SIZE];
bool toggle = true;

/* SLIP special character codes, written in octal */
#define END             0300    /* indicates end of packet */
#define ESC             0333    /* indicates byte stuffing */
#define ESC_END         0334    /* ESC ESC_END means END data byte */
#define ESC_ESC         0335    /* ESC ESC_ESC means ESC data byte */

int main(void) {
  int i;
  for (i = 0; i < BUF_SIZE; i++) {
    packet[i] = 'a';
  }
  gpio_enable_output(0);
  radio_init();

  printf("START 802.15.4 TRANSMIT\n");

  printf("END:     %x\n", END);
  printf("ESC:     %x\n", ESC);
  printf("ESC_END: %x\n", ESC_END);
  printf("ESC_ESC: %x\n", ESC_ESC);

  radio_set_addr(0x1540);
  radio_set_pan(0xABCD);
  radio_commit();             // START HERE
  while (1) {
    led_toggle(0);
    int err = radio_send(0x0802, packet, BUF_SIZE);
    if (err != TOCK_SUCCESS) {
      gpio_toggle(0);
    }
    delay_ms(250);
  }
}
