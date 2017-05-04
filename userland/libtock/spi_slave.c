#include "spi.h"

#define SPI_SLAVE 18

__attribute__((const)) int spi_slave_init(void) {return 0;}
int spi_slave_set_chip_select(unsigned char cs) {return command(SPI_SLAVE, 3, cs);}
int spi_slave_get_chip_select(void)             {return command(SPI_SLAVE, 4, 0);}
int spi_slave_set_rate(int rate)                {return command(SPI_SLAVE, 5, rate);}
int spi_slave_get_rate(void)                    {return command(SPI_SLAVE, 6, 0);}
int spi_slave_set_phase(bool phase)             {return command(SPI_SLAVE, 7, (unsigned char)phase);}
int spi_slave_get_phase(void)                   {return command(SPI_SLAVE, 8, 0);}
int spi_slave_set_polarity(bool pol)            {return command(SPI_SLAVE, 9, (unsigned char)pol);}
int spi_slave_get_polarity(void)                {return command(SPI_SLAVE, 10, 0);}
int spi_slave_hold_low(void)                    {return command(SPI_SLAVE, 11, 0);}
int spi_slave_release_low(void)                 {return command(SPI_SLAVE, 12, 0);}

/* This is no longer supported */
int spi_slave_write_byte(unsigned char byte) {
  return command(SPI_SLAVE, 1, byte);
}

/* This registers a callback for when the slave is selected. */
int spi_slave_chip_selected(subscribe_cb cb, bool* cond) {
  return subscribe(SPI_SLAVE, 1, cb, cond);
}

int spi_slave_read_buf(const char* str, size_t len) {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wcast-qual"
  // in lieu of RO allow
  void* buf = (void*) str;
#pragma GCC diagnostic pop
  return allow(SPI_SLAVE, 0, buf, len);
}

static void spi_slave_cb( __attribute__ ((unused)) int unused0,
                    __attribute__ ((unused)) int unused1,
                    __attribute__ ((unused)) int unused2,
                    __attribute__ ((unused)) void* ud) {
  *((bool*)ud) = true;
}

int spi_slave_write(const char* str,
              size_t len,
              subscribe_cb cb, bool* cond) {
  int err;
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wcast-qual"
  // in lieu of RO allow
  void* buf = (void*) str;
#pragma GCC diagnostic pop
  err = allow(SPI_SLAVE, 1, buf, len);
  if (err < 0 ) {
    return err;
  }
  err = subscribe(SPI_SLAVE, 0, cb, cond);
  if (err < 0 ) {
    return err;
  }
  return command(SPI_SLAVE, 2, len);
}

int spi_slave_read_write(const char* write,
                   char* read,
                   size_t  len,
                   subscribe_cb cb, bool* cond) {

  int err = allow(SPI_SLAVE, 0, (void*)read, len);
  if (err < 0) {
    return err;
  }
  return spi_slave_write(write, len, cb, cond);
}

int spi_slave_write_sync(const char* write,
                   size_t  len) {
  bool cond = false;
  spi_slave_write(write, len, spi_slave_cb, &cond);
  yield_for(&cond);
  return 0;
}

int spi_slave_read_write_sync(const char* write,
                        char* read,
                        size_t  len) {
  bool cond = false;
  int err = spi_slave_read_write(write, read, len, spi_slave_cb, &cond);
  if (err < 0) {
    return err;
  }
  yield_for(&cond);
  return 0;
}
