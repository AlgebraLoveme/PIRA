#include <stdio.h>
#include "../include/model.h"

static int twice(int value) {
    return value * 2;
}

int main(void) {
    Model model = {21};
    printf("%d\n", twice(model_value(&model)));
    return 0;
}
