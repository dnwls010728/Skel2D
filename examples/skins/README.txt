Skin (outfit) example: one skeleton and three outfits (knight, mage, ranger).

- Open skins.skel.json in the editor, step 2, Outfits (skin), and press the eye button of an outfit.
- Animations: idle, walk, wave. They play the same with every outfit.
- Rebuild from scratch:
    skel2d new skins.skel.json --name skins --force
    skel2d apply skins.skel.json build.ops.json
- images/ was cut from source/parts_sheet_2.png, an AI-generated sheet, with:
    skel2d slice source/parts_sheet_2.png -o images --key 00ff00 --key-tolerance 90 --defringe 32
