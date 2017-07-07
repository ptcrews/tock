#include <stdbool.h>
#include <stdio.h>

#include <led.h>
#include <radio.h>
#include <timer.h>

static void slip_send_packet(char *p, int len);

/** RADIO RECEIVE **/

#define BUF_SIZE 60
char packet_rx[BUF_SIZE];
char packet_tx[BUF_SIZE];
bool toggle = true;

static void callback(__attribute__ ((unused)) int unused0,
                     __attribute__ ((unused)) int unused1,
                     __attribute__ ((unused)) int unused2,
                     __attribute__ ((unused)) void* ud) {
  led_toggle(0);

  slip_send_packet (packet_rx, BUF_SIZE);

  radio_receive_callback(callback, packet_rx, BUF_SIZE);
}

/** SERIAL TRANSMIT **/

/* Uses serial line IP (SLIP) as specified in RFC 1055. */

/* SLIP special character codes */
#define END             0300    /* indicates end of packet */
#define ESC             0333    /* indicates byte stuffing */
#define ESC_END         0334    /* ESC ESC_END means END data byte */
#define ESC_ESC         0335    /* ESC ESC_ESC means ESC data byte */

static void send_char(char c) {
  printf("%c", c);
}

/* SEND_PACKET: sends a packet of length "len", starting at
 * location "p".
 */
static void slip_send_packet(char *p, int len) {

 /* send an initial END character to flush out any data that may
  * have accumulated in the receiver due to line noise
  */
  send_char(END);

 /* for each byte in the packet, send the appropriate character
  * sequence
  */
  while(len--) {
    switch(*p) {
    /* if it's the same code as an END character, we send a
    * special two character code so as not to make the
    * receiver think we sent an END
    */
    case END:
      send_char(ESC);
      send_char(ESC_END);
      break;

    /* if it's the same code as an ESC character,
    * we send a special two character code so as not
    * to make the receiver think we sent an ESC
    */
    case ESC:
      send_char(ESC);
      send_char(ESC_ESC);
      break;

    /* otherwise, we just send the character
    */
    default:
      send_char(*p);
    }

    p++;
  }

  /* tell the receiver that we're done sending the packet
  */
  send_char(END);
}

int main(void) {
  int i;

  printf("START 802.15.4 RECEIVE\n");

  for (i = 0; i < BUF_SIZE; i++) {
    packet_rx[i] = packet_tx[i] = i;
  }
  radio_set_addr(0x0802);
  radio_set_pan(0xABCD);
  radio_commit();
  radio_receive_callback(callback, packet_rx, BUF_SIZE);
  while (1) {
    delay_ms(4000);
  }
}
