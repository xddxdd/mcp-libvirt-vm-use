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


/*----------------------------------------------------------------------------
**  bitmap.js
**      Handle SPICE_IMAGE_TYPE_BITMAP
**--------------------------------------------------------------------------*/

import { Constants } from './enums.js';

/* A 32-bit source pixel is B, G, R, A in memory. Read it as a little-endian
   word, A R G B, rotate it left by eight to R G B A and store that word
   big-endian, so the bytes land in the R, G, B, A order ImageData wants: one
   load, one store and two shifts per pixel. A DataView takes either host
   byte order and any alignment. Only 32BIT and RGBA are handled; 32BIT
   ignores the source's high byte and is fully opaque. */
function convert_spice_bitmap_to_web(context, spice_bitmap)
{
    if (spice_bitmap.format != Constants.SPICE_BITMAP_FMT_32BIT &&
        spice_bitmap.format != Constants.SPICE_BITMAP_FMT_RGBA)
        return undefined;

    const w = spice_bitmap.x;
    const h = spice_bitmap.y;
    const stride = spice_bitmap.stride;
    const ret = context.createImageData(w, h);
    const opaque = spice_bitmap.format == Constants.SPICE_BITMAP_FMT_32BIT;
    const top_down = spice_bitmap.flags & Constants.SPICE_BITMAP_FLAGS_TOP_DOWN;
    const src = new DataView(spice_bitmap.data);
    const dest = new DataView(ret.data.buffer);
    let d = 0;
    for (let y = 0; y < h; y++)
    {
        let s = (top_down ? y : h - 1 - y) * stride;
        for (let x = 0; x < w; x++, d += 4, s += 4)
        {
            const v = src.getUint32(s, true);
            dest.setUint32(d, (v << 8) | (opaque ? 0xff : v >>> 24), false);
        }
    }
    return ret;
}

export {
  convert_spice_bitmap_to_web,
};
