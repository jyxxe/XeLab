#include <xelab/sdk.h>

int main() {
  return xelab::sdk::IsUuid("00000000-0000-4000-8000-000000000001") ? 0 : 1;
}
